//! Read-only JSON-RPC client: cache first, then network with retry and
//! endpoint failover.
//!
//! This client issues eth_chainId, eth_call, eth_getCode, eth_getBlockByNumber,
//! eth_getLogs and eth_getTransactionByHash, plus web3_clientVersion to
//! fingerprint an endpoint for meta.json. There is no code path here that can
//! send a transaction.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::cache::{cache_key, Cache, CacheMeta};
use crate::util::now_utc;

pub const DEFAULT_ARCHIVE_ENDPOINT: &str = "https://eth.drpc.org";
pub const DEFAULT_LATEST_ENDPOINT: &str = "https://ethereum.publicnode.com";
/// Blockscout's keyless etherscan compatible API. Used for log history only.
pub const DEFAULT_LOG_HISTORY_ENDPOINT: &str = "https://eth.blockscout.com/api";

/// The endpoint identity that is allowed into evidence and cache metadata.
///
/// RPC providers commonly carry the API key in the URL itself (a path
/// segment, a query parameter, or userinfo). A bundle is meant to be shared,
/// so the full URL never leaves process memory: userinfo, query and fragment
/// are dropped, and any path segment that looks like a key (16 characters or
/// more, or 8 or more with a digit in it) is replaced by `<redacted>`. What
/// remains still identifies the provider and route, which is all a reader of
/// the evidence needs.
pub fn redact_endpoint(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, url),
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let mut out = String::new();
    if let Some(scheme) = scheme {
        out.push_str(scheme);
        out.push_str("://");
    }
    out.push_str(host);
    if let Some(path) = path {
        for segment in path.split('/') {
            out.push('/');
            if segment.is_empty() {
                continue;
            }
            let has_digit = segment.chars().any(|c| c.is_ascii_digit());
            let key_like = segment.len() >= 16 || (segment.len() >= 8 && has_digit);
            out.push_str(if key_like { "<redacted>" } else { segment });
        }
    }
    out
}

/// Blockscout returns at most this many logs per request. It does not signal
/// truncation: a capped response looks exactly like a complete one, and the
/// `page` parameter is ignored on this instance (verified 2026-08-28, pages
/// 1, 2 and 3 all returned the identical first 1000 rows). A response at the
/// cap must therefore be treated as incomplete and the block window narrowed.
pub const BLOCKSCOUT_RESULT_CAP: usize = 1000;

// The retry budget is deliberately short. Observed against eth.drpc.org, a
// chunk that the endpoint refuses keeps being refused for minutes, while the
// same range split in half is served immediately. Patience does not recover
// those; narrowing does, and the caller can only narrow once this gives up.
// A long backoff here just makes a two million block sweep take hours.
const MAX_ATTEMPTS: u32 = 5;
const BASE_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 8_000;

/// Identity of a single read. The first five fields are exactly the cache key
/// inputs; `params` is the literal JSON-RPC params array and `label` is human
/// documentation. Neither of the latter two takes part in the key.
/// How a descriptor is put on the wire.
#[derive(Debug, Clone)]
pub enum Wire {
    /// POST the JSON-RPC body to each configured RPC endpoint.
    JsonRpc,
    /// GET the endpoint with these query parameters appended. `json` is
    /// false for a plain text body such as a CSV, where any HTTP 200 is a
    /// successful answer and the body is cached verbatim without parsing.
    HttpGet {
        query: Vec<(String, String)>,
        json: bool,
        /// Absolute base URL for a source that is not one of the configured
        /// log endpoints, such as the Treasury CSV. None means use the
        /// configured log endpoint list.
        base: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Descriptor {
    pub label: String,
    pub wire: Wire,
    pub method: String,
    pub block: String,
    pub to: String,
    pub calldata: String,
    pub params: Value,
}

impl Descriptor {
    /// The request as recorded in the manifest. For a GET this is a
    /// description of the query rather than a body, because there is no body.
    pub fn request_body(&self) -> Value {
        match &self.wire {
            Wire::JsonRpc => json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": self.method,
                "params": self.params,
            }),
            Wire::HttpGet { query, .. } => json!({
                "http_method": "GET",
                "api": self.method,
                "query": query
                    .iter()
                    .map(|(k, v)| json!([k, v]))
                    .collect::<Vec<Value>>(),
            }),
        }
    }

    fn query_string(&self) -> String {
        match &self.wire {
            Wire::JsonRpc => String::new(),
            Wire::HttpGet { query, .. } => query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<String>>()
                .join("&"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub descriptor: Descriptor,
    pub key: String,
    /// The response body exactly as the node sent it.
    pub body: String,
    pub cache_hit: bool,
    /// The endpoint that produced this body, including on a cache hit, where
    /// it is read back from the cache metadata.
    pub endpoint: String,
    pub stored_utc: String,
}

impl Fetched {
    pub fn parsed(&self) -> Result<Value, String> {
        serde_json::from_str(&self.body).map_err(|err| format!("response is not JSON: {err}"))
    }

    /// The `result` field, or an error describing the JSON-RPC error object.
    pub fn result(&self) -> Result<Value, String> {
        let parsed = self.parsed()?;
        if let Some(result) = parsed.get("result") {
            return Ok(result.clone());
        }
        Err(describe_rpc_error(&parsed))
    }

    pub fn result_str(&self) -> Result<String, String> {
        match self.result()? {
            Value::String(s) => Ok(s),
            other => Err(format!("expected a string result, got {other}")),
        }
    }
}

/// A retry, failover or rate limit event, recorded so the report can state the
/// limits actually observed rather than the limits the docs claim.
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub utc: String,
    pub endpoint: String,
    pub method: String,
    pub label: String,
    pub attempt: u32,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub enum RpcErrorKind {
    /// Range limits and similar: retrying the identical request is pointless,
    /// the caller has to change the request.
    RequestTooBroad,
    /// Exhausted retries, or a hard refusal such as "archive not supported".
    Failed,
    /// Cache miss while running with --offline.
    OfflineMiss,
}

#[derive(Debug, Clone)]
pub struct RpcError {
    pub kind: RpcErrorKind,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

fn describe_rpc_error(parsed: &Value) -> String {
    match parsed.get("error") {
        Some(error) => {
            let code = error.get("code").and_then(Value::as_i64);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("(no message)");
            match code {
                Some(code) => format!("JSON-RPC error {code}: {message}"),
                None => format!("JSON-RPC error: {message}"),
            }
        }
        None => "response has neither result nor error".to_string(),
    }
}

/// How to treat a body that came back with HTTP 200.
#[derive(Debug, PartialEq)]
enum Classification {
    /// A `result` field is present.
    Success,
    /// A revert at a pinned block is a deterministic property of the chain,
    /// so it is cached like any other answer and reported as a finding.
    DeterministicRevert,
    /// Rate limits and transient node errors.
    Retryable,
    /// Range limits: the caller must narrow the request.
    RequestTooBroad,
    /// Anything else, for example "archive state not available on this node".
    Fatal,
}

fn classify(body: &str) -> (Classification, String) {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(parsed) => parsed,
        Err(err) => return (Classification::Retryable, format!("unparsable body: {err}")),
    };

    // Blockscout's etherscan compatible API answers with status/message/result
    // and no jsonrpc field. status "0" is used both for real errors and for
    // the ordinary empty result, so the message has to be read.
    if parsed.get("jsonrpc").is_none() {
        if let Some(status) = parsed.get("status").and_then(Value::as_str) {
            let message = parsed
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if status == "1" {
                return (Classification::Success, String::new());
            }
            if message.contains("no logs found") || message.contains("no records found") {
                return (Classification::Success, String::new());
            }
            let detail = format!("Blockscout status {status}: {message}");
            if message.contains("rate limit")
                || message.contains("throttl")
                || message.contains("try again")
            {
                return (Classification::Retryable, detail);
            }
            return (Classification::Fatal, detail);
        }
    }

    if parsed.get("result").is_some() {
        return (Classification::Success, String::new());
    }
    let description = describe_rpc_error(&parsed);
    let code = parsed
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_i64);
    let message = parsed
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();

    if code == Some(3) || message.contains("execution reverted") || message.contains("revert") {
        return (Classification::DeterministicRevert, description);
    }
    // Rate limits are checked before range limits: "rate limit exceeded" and
    // "ranges over 10000 blocks" both contain the word "limit", and only the
    // second one means the request itself has to change.
    if message.contains("rate limit")
        || message.contains("too many requests")
        || message.contains("capacity")
        || message.contains("timeout")
        || message.contains("try again")
        // Observed from eth.drpc.org on the free tier: a chunk that is inside
        // the documented range limit, and that the same endpoint served 175
        // times in the same run, can come back as "method handler crashed".
        // It is a backend condition, not a property of the request.
        || message.contains("crashed")
        || message.contains("internal error")
        || code == Some(-32000)
        || code == Some(-32005)
        || code == Some(-32603)
    {
        return (Classification::Retryable, description);
    }
    if message.contains("range")
        || message.contains("too many results")
        || message.contains("query returned more than")
        || message.contains("response size")
        || message.contains("too large")
    {
        return (Classification::RequestTooBroad, description);
    }
    (Classification::Fatal, description)
}

/// Where reads come from. The network client and the bundle-backed source
/// (`source::BundleSource`) both implement it, so a run function recomputes
/// from either without knowing which (spec 03 R6).
pub trait ReadSource {
    /// One read, by descriptor: the body verbatim, from wherever the source
    /// holds it.
    fn fetch(&mut self, descriptor: Descriptor) -> Result<Fetched, RpcError>;
    /// The chain the source is bound to. Cache keys are computed under it.
    fn chain_id(&self) -> u64;
    /// (network calls, cache or bundle hits) so far.
    fn counters(&self) -> (usize, usize);
    /// What the source is, for meta.json: endpoints, counters, observations,
    /// fingerprints. Never a credential.
    fn meta(&self) -> Value;
}

impl ReadSource for Client {
    fn fetch(&mut self, descriptor: Descriptor) -> Result<Fetched, RpcError> {
        Client::fetch(self, descriptor)
    }

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn counters(&self) -> (usize, usize) {
        (self.network_calls, self.cache_hits)
    }

    fn meta(&self) -> Value {
        json!({
            "source": "network",
            "endpoints_configured": self.endpoints(),
            "log_endpoints_configured": self.log_endpoints(),
            "cache_root": self.cache.root().display().to_string(),
            "offline": self.offline,
            "network_calls_this_run": self.network_calls,
            "cache_hits_this_run": self.cache_hits,
            "rpc_observations": self.observations,
            "endpoint_fingerprints": self.endpoint_fingerprints(),
        })
    }
}

/// One endpoint that served at least one body this run: when it was first
/// used and whether it speaks JSON-RPC (so it can be asked for its chain id
/// and client version).
#[derive(Debug, Clone)]
pub struct Served {
    pub first_used_utc: String,
    pub json_rpc: bool,
}

pub struct Client {
    agent: ureq::Agent,
    endpoints: Vec<String>,
    log_endpoints: Vec<String>,
    cache: Cache,
    chain_id: u64,
    offline: bool,
    pacing: Option<Duration>,
    /// Full URLs, private to the client; redacted on the way out.
    served: BTreeMap<String, Served>,
    pub network_calls: usize,
    pub cache_hits: usize,
    pub observations: Vec<Observation>,
}

impl Client {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoints: Vec<String>,
        log_endpoints: Vec<String>,
        cache: Cache,
        chain_id: u64,
        offline: bool,
        pacing_ms: u64,
    ) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .build();
        Self {
            agent,
            endpoints,
            log_endpoints,
            cache,
            chain_id,
            offline,
            pacing: (pacing_ms > 0).then(|| Duration::from_millis(pacing_ms)),
            served: BTreeMap::new(),
            network_calls: 0,
            cache_hits: 0,
            observations: Vec::new(),
        }
    }

    /// The configured JSON-RPC endpoints, redacted for evidence. The full
    /// URLs stay private to the client.
    pub fn endpoints(&self) -> Vec<String> {
        self.endpoints
            .iter()
            .map(|url| redact_endpoint(url))
            .collect()
    }

    /// The configured log history endpoints, redacted for evidence.
    pub fn log_endpoints(&self) -> Vec<String> {
        self.log_endpoints
            .iter()
            .map(|url| redact_endpoint(url))
            .collect()
    }

    fn endpoints_for(&self, descriptor: &Descriptor) -> Vec<String> {
        match &descriptor.wire {
            Wire::JsonRpc => self.endpoints.clone(),
            Wire::HttpGet {
                base: Some(base), ..
            } => vec![base.clone()],
            Wire::HttpGet { base: None, .. } => self.log_endpoints.clone(),
        }
    }

    /// Spec 03 R3: one fingerprint per endpoint that served a body this run.
    /// JSON-RPC endpoints are asked directly for eth_chainId and
    /// web3_clientVersion; the answers are not cached and not evidence, so
    /// they go to meta.json only. A refusal leaves the field null.
    pub fn endpoint_fingerprints(&self) -> Vec<Value> {
        let agent = &self.agent;
        let probe = |url: &str| -> (Option<u64>, Option<String>) {
            let ask = |method: &str| -> Option<Value> {
                let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": []});
                let response = agent
                    .post(url)
                    .set("content-type", "application/json")
                    .send_string(&body.to_string())
                    .ok()?;
                let parsed: Value = serde_json::from_str(&response.into_string().ok()?).ok()?;
                parsed.get("result").cloned()
            };
            let chain_id = ask("eth_chainId")
                .and_then(|v| v.as_str().map(str::to_string))
                .and_then(|hex| crate::util::parse_hex_u64(&hex));
            let version = ask("web3_clientVersion").and_then(|v| v.as_str().map(str::to_string));
            (chain_id, version)
        };
        fingerprint_entries(&self.served, probe)
    }

    fn observe(
        &mut self,
        endpoint: &str,
        descriptor: &Descriptor,
        attempt: u32,
        kind: &str,
        detail: String,
    ) {
        self.observations.push(Observation {
            utc: now_utc(),
            endpoint: redact_endpoint(endpoint),
            method: descriptor.method.clone(),
            label: descriptor.label.clone(),
            attempt,
            kind: kind.to_string(),
            detail,
        });
    }

    pub fn fetch(&mut self, descriptor: Descriptor) -> Result<Fetched, RpcError> {
        let key = cache_key(self.chain_id, &descriptor);
        if let Some((body, meta)) = self.cache.get(&key) {
            self.cache_hits += 1;
            return Ok(Fetched {
                descriptor,
                key,
                body,
                cache_hit: true,
                // Older cache entries may carry the full URL; never let it
                // through into a bundle.
                endpoint: redact_endpoint(&meta.endpoint),
                stored_utc: meta.stored_utc,
            });
        }
        if self.offline {
            return Err(RpcError {
                kind: RpcErrorKind::OfflineMiss,
                message: format!(
                    "--offline was requested but {} ({}) is not in the cache (key {key})",
                    descriptor.label, descriptor.method
                ),
            });
        }
        self.fetch_from_network(descriptor, key)
    }

    fn fetch_from_network(
        &mut self,
        descriptor: Descriptor,
        key: String,
    ) -> Result<Fetched, RpcError> {
        let request_body = descriptor.request_body();
        let request_text = request_body.to_string();
        let endpoints = self.endpoints_for(&descriptor);
        let mut last_detail: Vec<String> = vec![String::new(); endpoints.len()];

        for attempt in 0..MAX_ATTEMPTS {
            for (endpoint_index, endpoint) in endpoints.iter().enumerate() {
                if let Some(delay) = self.pacing {
                    sleep(delay);
                }
                self.network_calls += 1;
                let response = match &descriptor.wire {
                    Wire::JsonRpc => self
                        .agent
                        .post(endpoint)
                        .set("content-type", "application/json")
                        .send_string(&request_text),
                    Wire::HttpGet { .. } => {
                        let separator = if endpoint.contains('?') { "&" } else { "?" };
                        let url = format!("{endpoint}{separator}{}", descriptor.query_string());
                        self.agent.get(&url).call()
                    }
                };

                let (status, body, retry_after) = match response {
                    Ok(response) => {
                        let status = response.status();
                        let retry_after = response
                            .header("retry-after")
                            .and_then(|value| value.parse::<u64>().ok());
                        match response.into_string() {
                            Ok(body) => (status, body, retry_after),
                            Err(err) => {
                                let detail = format!("could not read response body: {err}");
                                last_detail[endpoint_index] = detail.clone();
                                self.observe(
                                    endpoint,
                                    &descriptor,
                                    attempt,
                                    "body_read_failed",
                                    detail,
                                );
                                continue;
                            }
                        }
                    }
                    Err(ureq::Error::Status(status, response)) => {
                        let retry_after = response
                            .header("retry-after")
                            .and_then(|value| value.parse::<u64>().ok());
                        let body = response.into_string().unwrap_or_default();
                        (status, body, retry_after)
                    }
                    Err(ureq::Error::Transport(transport)) => {
                        let detail = format!("transport error: {transport}");
                        last_detail[endpoint_index] = detail.clone();
                        self.observe(endpoint, &descriptor, attempt, "transport_error", detail);
                        continue;
                    }
                };

                if status == 429 || (500..600).contains(&status) {
                    let detail = format!("HTTP {status}: {}", truncate(&body, 300));
                    last_detail[endpoint_index] = detail.clone();
                    let kind = if status == 429 {
                        "http_429_rate_limited"
                    } else {
                        "http_5xx"
                    };
                    self.observe(endpoint, &descriptor, attempt, kind, detail);
                    if let Some(seconds) = retry_after {
                        sleep(Duration::from_secs(seconds.min(30)));
                    }
                    continue;
                }
                if status != 200 {
                    let detail = format!("HTTP {status}: {}", truncate(&body, 300));
                    last_detail[endpoint_index] = detail.clone();
                    self.observe(endpoint, &descriptor, attempt, "http_error", detail);
                    continue;
                }

                let (classification, detail) = match &descriptor.wire {
                    // A plain text body such as a CSV: any non-empty HTTP 200
                    // is the answer.
                    Wire::HttpGet { json: false, .. } => {
                        if body.trim().is_empty() {
                            (Classification::Retryable, "empty body".to_string())
                        } else {
                            (Classification::Success, String::new())
                        }
                    }
                    // A JSON GET against a source that is not JSON-RPC, such
                    // as DefiLlama: the JSON-RPC result/error shape does not
                    // apply, so a parseable body without an error field is the
                    // answer.
                    Wire::HttpGet {
                        json: true,
                        base: Some(_),
                        ..
                    } => match serde_json::from_str::<Value>(&body) {
                        Ok(parsed) => {
                            if let Some(error) = parsed.get("error") {
                                (
                                    Classification::Fatal,
                                    format!("source returned an error: {error}"),
                                )
                            } else {
                                (Classification::Success, String::new())
                            }
                        }
                        Err(err) => (Classification::Retryable, format!("unparsable body: {err}")),
                    },
                    _ => classify(&body),
                };
                match classification {
                    Classification::Success | Classification::DeterministicRevert => {
                        if classification == Classification::DeterministicRevert {
                            self.observe(
                                endpoint,
                                &descriptor,
                                attempt,
                                "deterministic_revert",
                                detail,
                            );
                        }
                        self.served
                            .entry(endpoint.clone())
                            .or_insert_with(|| Served {
                                first_used_utc: now_utc(),
                                json_rpc: matches!(descriptor.wire, Wire::JsonRpc),
                            });
                        let meta = CacheMeta {
                            key: key.clone(),
                            chain_id: self.chain_id,
                            method: descriptor.method.clone(),
                            block: descriptor.block.clone(),
                            to: descriptor.to.clone(),
                            calldata: descriptor.calldata.clone(),
                            request: request_body.clone(),
                            endpoint: redact_endpoint(endpoint),
                            stored_utc: now_utc(),
                        };
                        self.cache.put(&key, &body, &meta).map_err(|err| RpcError {
                            kind: RpcErrorKind::Failed,
                            message: format!("could not write cache entry {key}: {err}"),
                        })?;
                        return Ok(Fetched {
                            descriptor,
                            key,
                            body,
                            cache_hit: false,
                            endpoint: meta.endpoint.clone(),
                            stored_utc: meta.stored_utc,
                        });
                    }
                    Classification::RequestTooBroad => {
                        self.observe(
                            endpoint,
                            &descriptor,
                            attempt,
                            "request_too_broad",
                            detail.clone(),
                        );
                        return Err(RpcError {
                            kind: RpcErrorKind::RequestTooBroad,
                            message: detail,
                        });
                    }
                    Classification::Retryable => {
                        last_detail[endpoint_index] = detail.clone();
                        self.observe(endpoint, &descriptor, attempt, "rpc_retryable", detail);
                        continue;
                    }
                    Classification::Fatal => {
                        last_detail[endpoint_index] = detail.clone();
                        self.observe(endpoint, &descriptor, attempt, "rpc_refused", detail);
                        continue;
                    }
                }
            }

            let backoff = (BASE_BACKOFF_MS << attempt).min(MAX_BACKOFF_MS);
            sleep(Duration::from_millis(backoff));
        }

        let per_endpoint: Vec<String> = endpoints
            .iter()
            .zip(last_detail.iter())
            .map(|(endpoint, detail)| {
                let detail = if detail.is_empty() {
                    "no error recorded"
                } else {
                    detail
                };
                format!("{} said: {detail}", redact_endpoint(endpoint))
            })
            .collect();
        Err(RpcError {
            kind: RpcErrorKind::Failed,
            message: format!(
                "{} ({}) failed on every endpoint after {MAX_ATTEMPTS} attempts; {}",
                descriptor.label,
                descriptor.method,
                per_endpoint.join(" | ")
            ),
        })
    }
}

/// The fingerprint list for meta.json, from the served map and a probe. The
/// probe sees the full URL (it has to reach the endpoint); only the redacted
/// form is written. Non-JSON-RPC endpoints are listed without a probe.
pub fn fingerprint_entries(
    served: &BTreeMap<String, Served>,
    probe: impl Fn(&str) -> (Option<u64>, Option<String>),
) -> Vec<Value> {
    served
        .iter()
        .map(|(url, served)| {
            let (chain_id, client_version) = if served.json_rpc {
                probe(url)
            } else {
                (None, None)
            };
            json!({
                "endpoint": redact_endpoint(url),
                "wire": if served.json_rpc { "json_rpc" } else { "http_get" },
                "chain_id": chain_id,
                "client_version": client_version,
                "first_used_utc": served.first_used_utc,
            })
        })
        .collect()
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    format!("{}...", &text[..max])
}

// ---------------------------------------------------------------------------
// Descriptor constructors, one per JSON-RPC method used
// ---------------------------------------------------------------------------

pub fn chain_id_descriptor() -> Descriptor {
    Descriptor {
        label: "eth_chainId".to_string(),
        wire: Wire::JsonRpc,
        method: "eth_chainId".to_string(),
        block: "n/a".to_string(),
        to: String::new(),
        calldata: String::new(),
        params: json!([]),
    }
}

pub fn call_descriptor(label: &str, to: &str, calldata: &str, block_hex: &str) -> Descriptor {
    Descriptor {
        label: label.to_string(),
        wire: Wire::JsonRpc,
        method: "eth_call".to_string(),
        block: block_hex.to_string(),
        to: to.to_string(),
        calldata: calldata.to_string(),
        params: json!([{ "to": to, "data": calldata }, block_hex]),
    }
}

pub fn get_code_descriptor(label: &str, address: &str, block_hex: &str) -> Descriptor {
    Descriptor {
        label: label.to_string(),
        wire: Wire::JsonRpc,
        method: "eth_getCode".to_string(),
        block: block_hex.to_string(),
        to: address.to_string(),
        calldata: String::new(),
        params: json!([address, block_hex]),
    }
}

pub fn get_block_descriptor(label: &str, block_hex: &str) -> Descriptor {
    Descriptor {
        label: label.to_string(),
        wire: Wire::JsonRpc,
        method: "eth_getBlockByNumber".to_string(),
        block: block_hex.to_string(),
        to: String::new(),
        calldata: "full_transactions=false".to_string(),
        params: json!([block_hex, false]),
    }
}

/// For eth_getLogs the cache key slots are reused: `block` holds the inclusive
/// range and `calldata` holds the topic filter, so two different ranges or two
/// different filters can never collide on one key.
pub fn get_logs_descriptor(label: &str, address: &str, from_hex: &str, to_hex: &str) -> Descriptor {
    Descriptor {
        label: label.to_string(),
        wire: Wire::JsonRpc,
        method: "eth_getLogs".to_string(),
        block: format!("{from_hex}..{to_hex}"),
        to: address.to_string(),
        calldata: "topics=none".to_string(),
        params: json!([{
            "address": address,
            "fromBlock": from_hex,
            "toBlock": to_hex,
        }]),
    }
}

/// Blockscout log history request. The cache key slots follow the same
/// convention as eth_getLogs: `block` holds the inclusive range, `calldata`
/// holds the topic filter.
pub fn blockscout_logs_descriptor(
    label: &str,
    address: &str,
    topic0: Option<&str>,
    topic1: Option<&str>,
    from_block: u64,
    to_block: u64,
) -> Descriptor {
    blockscout_logs_descriptor_full(label, address, topic0, topic1, None, from_block, to_block)
}

/// As above, plus the second indexed topic. Blockscout needs an explicit
/// operator for every pair of topic slots that is supplied.
#[allow(clippy::too_many_arguments)]
pub fn blockscout_logs_descriptor_full(
    label: &str,
    address: &str,
    topic0: Option<&str>,
    topic1: Option<&str>,
    topic2: Option<&str>,
    from_block: u64,
    to_block: u64,
) -> Descriptor {
    let mut query = vec![
        ("module".to_string(), "logs".to_string()),
        ("action".to_string(), "getLogs".to_string()),
        ("address".to_string(), address.to_string()),
        ("fromBlock".to_string(), from_block.to_string()),
        ("toBlock".to_string(), to_block.to_string()),
    ];
    if let Some(topic0) = topic0 {
        query.push(("topic0".to_string(), topic0.to_string()));
    }
    if let Some(topic1) = topic1 {
        query.push(("topic1".to_string(), topic1.to_string()));
    }
    if let Some(topic2) = topic2 {
        query.push(("topic2".to_string(), topic2.to_string()));
    }
    if topic0.is_some() && topic1.is_some() {
        query.push(("topic0_1_opr".to_string(), "and".to_string()));
    }
    if topic0.is_some() && topic2.is_some() {
        query.push(("topic0_2_opr".to_string(), "and".to_string()));
    }
    if topic1.is_some() && topic2.is_some() {
        query.push(("topic1_2_opr".to_string(), "and".to_string()));
    }
    // The topic filter goes in the calldata key slot, so a filtered and an
    // unfiltered fetch over the same range can never share a cache key.
    let filter = format!(
        "topic0={},topic1={},topic2={}",
        topic0.unwrap_or("none"),
        topic1.unwrap_or("none"),
        topic2.unwrap_or("none")
    );
    Descriptor {
        label: label.to_string(),
        wire: Wire::HttpGet {
            query: query.clone(),
            json: true,
            base: None,
        },
        method: "blockscout_getLogs".to_string(),
        block: format!("0x{from_block:x}..0x{to_block:x}"),
        to: address.to_string(),
        calldata: filter,
        params: json!(query),
    }
}

/// eth_getTransactionByHash. Not block pinned: a mined transaction is
/// immutable, so the hash alone identifies it.
pub fn get_transaction_descriptor(label: &str, hash: &str) -> Descriptor {
    Descriptor {
        label: label.to_string(),
        wire: Wire::JsonRpc,
        method: "eth_getTransactionByHash".to_string(),
        block: "mined".to_string(),
        to: hash.to_string(),
        calldata: String::new(),
        params: json!([hash]),
    }
}

/// A plain HTTP GET against a non-JSON-RPC source. `resource` names what is
/// being fetched and takes the cache key's `to` slot; the query takes the
/// calldata slot, so two different queries against one host cannot collide.
pub fn http_get_descriptor(
    label: &str,
    resource: &str,
    query: Vec<(String, String)>,
    json: bool,
    pin: &str,
) -> Descriptor {
    let rendered = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<String>>()
        .join("&");
    Descriptor {
        label: label.to_string(),
        wire: Wire::HttpGet {
            query,
            json,
            base: Some(format!("https://{resource}")),
        },
        method: "http_get".to_string(),
        // Not block pinned. The pin string says what it is pinned to instead,
        // and the bundle records the fetch timestamp.
        block: pin.to_string(),
        to: resource.to_string(),
        calldata: rendered,
        params: json!([]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_drops_keys_wherever_providers_put_them() {
        // Path segment keys (Alchemy, Infura, QuickNode, Ankr shapes).
        assert_eq!(
            redact_endpoint(
                "https://eth-mainnet.g.alchemy.com/v2/AbCdEf0123456789AbCdEf0123456789"
            ),
            "https://eth-mainnet.g.alchemy.com/v2/<redacted>"
        );
        assert_eq!(
            redact_endpoint("https://mainnet.infura.io/v3/0123456789abcdef0123456789abcdef"),
            "https://mainnet.infura.io/v3/<redacted>"
        );
        assert_eq!(
            redact_endpoint("https://rpc.ankr.com/eth/0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9"),
            "https://rpc.ankr.com/eth/<redacted>"
        );
        // Query string keys (dRPC shape) and fragments vanish entirely.
        assert_eq!(
            redact_endpoint("https://lb.drpc.org/ogrpc?network=ethereum&dkey=SECRET"),
            "https://lb.drpc.org/ogrpc"
        );
        // Userinfo vanishes.
        assert_eq!(
            redact_endpoint("https://user:secret@rpc.example.org/mainnet"),
            "https://rpc.example.org/mainnet"
        );
        // Short route segments survive, so the provider and route stay legible.
        assert_eq!(
            redact_endpoint(DEFAULT_ARCHIVE_ENDPOINT),
            DEFAULT_ARCHIVE_ENDPOINT
        );
        assert_eq!(
            redact_endpoint(DEFAULT_LATEST_ENDPOINT),
            DEFAULT_LATEST_ENDPOINT
        );
        assert_eq!(
            redact_endpoint(DEFAULT_LOG_HISTORY_ENDPOINT),
            DEFAULT_LOG_HISTORY_ENDPOINT
        );
        assert_eq!(
            redact_endpoint("https://x.example/api/v1/eth"),
            "https://x.example/api/v1/eth"
        );
    }

    /// Spec 03 R3: fingerprints carry the chain id the endpoint reported and
    /// never the credential in its URL.
    #[test]
    fn endpoint_fingerprints_are_redacted_and_carry_the_chain_id() {
        let mut served = BTreeMap::new();
        served.insert(
            "https://lb.drpc.org/ogrpc?network=ethereum&dkey=SECRETKEY123".to_string(),
            Served {
                first_used_utc: "2026-01-01T00:00:00.000Z".to_string(),
                json_rpc: true,
            },
        );
        served.insert(
            "https://mainnet.infura.io/v3/0123456789abcdef0123456789abcdef".to_string(),
            Served {
                first_used_utc: "2026-01-01T00:00:01.000Z".to_string(),
                json_rpc: true,
            },
        );
        served.insert(
            "https://eth.blockscout.com/api?apikey=ALSOSECRET".to_string(),
            Served {
                first_used_utc: "2026-01-01T00:00:02.000Z".to_string(),
                json_rpc: false,
            },
        );
        let probe = |url: &str| -> (Option<u64>, Option<String>) {
            if url.contains("infura") {
                // The client version refused, the chain id answered.
                (Some(1), None)
            } else {
                (Some(1), Some("Geth/v1.14.0".to_string()))
            }
        };
        let entries = fingerprint_entries(&served, probe);
        assert_eq!(entries.len(), 3);
        let text = serde_json::to_string(&entries).unwrap();
        assert!(!text.contains("SECRET"), "{text}");
        assert!(!text.contains("0123456789abcdef"), "{text}");
        let by_endpoint = |endpoint: &str| -> &Value {
            entries
                .iter()
                .find(|e| e["endpoint"] == endpoint)
                .unwrap_or_else(|| panic!("{endpoint} is listed in {text}"))
        };
        let drpc = by_endpoint("https://lb.drpc.org/ogrpc");
        assert_eq!(drpc["chain_id"], 1);
        assert_eq!(drpc["client_version"], "Geth/v1.14.0");
        assert_eq!(drpc["first_used_utc"], "2026-01-01T00:00:00.000Z");
        let infura = by_endpoint("https://mainnet.infura.io/v3/<redacted>");
        assert_eq!(infura["chain_id"], 1);
        assert_eq!(infura["client_version"], Value::Null);
        let blockscout = by_endpoint("https://eth.blockscout.com/api");
        assert_eq!(blockscout["wire"], "http_get");
        assert_eq!(blockscout["chain_id"], Value::Null);
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = redact_endpoint("https://mainnet.infura.io/v3/0123456789abcdef0123456789abcdef");
        assert_eq!(redact_endpoint(&once), once);
    }

    #[test]
    fn revert_bodies_are_deterministic_not_retryable() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":3,"message":"execution reverted","data":"0x"}}"#;
        assert_eq!(classify(body).0, Classification::DeterministicRevert);
    }

    #[test]
    fn range_limits_ask_the_caller_to_narrow() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":35,"message":"ranges over 10000 blocks are not supported on free plan"}}"#;
        assert_eq!(classify(body).0, Classification::RequestTooBroad);
    }

    #[test]
    fn rate_limits_are_retryable() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32005,"message":"rate limit exceeded, try again"}}"#;
        assert_eq!(classify(body).0, Classification::Retryable);
    }

    #[test]
    fn results_are_successes() {
        let body = r#"{"id":1,"jsonrpc":"2.0","result":"0x1"}"#;
        assert_eq!(classify(body).0, Classification::Success);
    }

    #[test]
    fn blockscout_empty_result_is_a_success_not_an_error() {
        let body = r#"{"message":"No logs found","result":[],"status":"0"}"#;
        assert_eq!(classify(body).0, Classification::Success);
    }

    #[test]
    fn blockscout_ok_is_a_success() {
        let body = r#"{"message":"OK","result":[{"address":"0x1"}],"status":"1"}"#;
        assert_eq!(classify(body).0, Classification::Success);
    }

    /// A real Blockscout failure must not be mistaken for an empty result,
    /// which is what would happen if only the presence of `result` were
    /// checked.
    #[test]
    fn blockscout_error_is_not_a_success() {
        let body = r#"{"message":"Invalid address format","result":null,"status":"0"}"#;
        assert_eq!(classify(body).0, Classification::Fatal);
    }

    #[test]
    fn blockscout_descriptor_separates_filtered_from_unfiltered() {
        let filtered = blockscout_logs_descriptor("a", "0xabc", Some("0xd76d"), None, 0, 10);
        let unfiltered = blockscout_logs_descriptor("a", "0xabc", None, None, 0, 10);
        let by_account = blockscout_logs_descriptor("a", "0xabc", None, Some("0xe5f1"), 0, 10);
        let by_topic2 =
            blockscout_logs_descriptor_full("a", "0xabc", Some("0xd76d"), None, Some("0x0"), 0, 10);
        assert_ne!(by_topic2.calldata, filtered.calldata);
        assert_ne!(filtered.calldata, unfiltered.calldata);
        assert_ne!(by_account.calldata, unfiltered.calldata);
        assert_ne!(by_account.calldata, filtered.calldata);
        assert!(matches!(filtered.wire, Wire::HttpGet { .. }));
    }

    #[test]
    fn get_logs_range_is_part_of_the_key_slot() {
        let a = get_logs_descriptor("logs", "0xabc", "0x1", "0x2");
        let b = get_logs_descriptor("logs", "0xabc", "0x3", "0x4");
        assert_ne!(a.block, b.block);
    }
}
