//! `crossfoot consume`: the consumer agent of `05-consumer-agent.md`.
//!
//! Runs the three subgraph queries (or reads recorded responses with
//! `--replay`), joins the latest posted state per feed with
//! `site/data/feeds.json` by address, applies the freshness gates and the
//! decision table (`model::decision`), and writes `decisions/<stamp>/` with
//! the verbatim responses, `decisions.json` and `decisions.sha256`. Every
//! record repeats the provenance a third party needs to re-check it: the
//! deployment ID and its digest, the indexed block, the hash of every query
//! file, variable set and response, the feeds.json hash and the policy.
//!
//! The only network path is the HTTP POST of a GraphQL query. There is no
//! signing key and no on-chain write anywhere in this module.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::hex_encode;
use crate::cache::sha256_hex;
use crate::model::decision::{
    decide, BoundChangeRow, CrossfootEvidence, CrossfootRow, Decision, EligibilityPolicy, Family,
    FeedInputs, Head, LatestRound, Pinned, Policy, RateChangeRow, SubgraphEvidence, SubgraphFeed,
    TimelineEvidence, UncheckedRound, UnknownRound,
};
use crate::rpc::redact_endpoint;
use crate::util::{git_provenance, now_stamp, unix_to_utc};

pub const FORMAT: &str = "crossfoot-decisions-v1";
pub const ENV_SUBGRAPH_URL: &str = "CROSSFOOT_SUBGRAPH_URL";
pub const ENV_SUBGRAPH_KEY: &str = "CROSSFOOT_SUBGRAPH_KEY";

/// The four query files, in run order. `Head` gives the live head that the
/// other three are pinned to unless `--block` is given (05 corrections C1).
pub const QUERY_NAMES: [&str; 4] = ["Head", "FeedStatus", "WindowFindings", "FeedTimeline"];

#[derive(Args, Debug, Clone)]
pub struct ConsumeOpts {
    /// Subgraph query URL (Studio or gateway). Falls back to
    /// CROSSFOOT_SUBGRAPH_URL. A bearer key is read from
    /// CROSSFOOT_SUBGRAPH_KEY and never written to any file.
    #[arg(long)]
    pub subgraph: Option<String>,

    /// The renderer's feed table (00 A1).
    #[arg(long, default_value = "site/data/feeds.json")]
    pub feeds: PathBuf,

    /// Directory holding FeedStatus.graphql, WindowFindings.graphql and
    /// FeedTimeline.graphql, used verbatim.
    #[arg(long, default_value = "subgraph/queries")]
    pub queries: PathBuf,

    /// The Midas feed list; entries with kind "derived" are listed as
    /// wrappers and not decided. Missing file: no wrappers.
    #[arg(long, default_value = "config/midas-mainnet.json")]
    pub midas_config: PathBuf,

    /// The consumer's eligibility policy (config/policy-default.json when
    /// that file exists); its sha256 goes into every record.
    #[arg(long)]
    pub policy: Option<PathBuf>,

    /// Directory the run directory is created under.
    #[arg(long, default_value = "decisions")]
    pub out: PathBuf,

    /// Window for unchecked posts, unattributable rounds and bound changes.
    #[arg(long, default_value_t = 183)]
    pub window_days: u64,

    /// A posted feed whose last round is older than this at the indexed
    /// head is STALE.
    #[arg(long, default_value_t = 30)]
    pub stale_after_days: u64,

    /// Indexed head older than this against now is SUBGRAPH_STALE.
    #[arg(long, default_value_t = 900)]
    pub max_head_lag_seconds: u64,

    /// A derived feed whose Crossfoot result is older than this (at 7200
    /// blocks per day) is RESULT_STALE.
    #[arg(long, default_value_t = 30)]
    pub max_result_age_days: u64,

    /// Unix time used as "now"; the system clock when absent. Recorded.
    #[arg(long)]
    pub now: Option<i64>,

    /// Pin every query to this block. Without it the head is probed first
    /// and every query is pinned to that number.
    #[arg(long)]
    pub block: Option<u64>,

    /// Read responses/ from this directory instead of the network.
    #[arg(long)]
    pub replay: Option<PathBuf>,

    /// Product (or 0x address) whose full timeline is queried. Repeatable.
    #[arg(long = "timeline", default_value = "mRE7")]
    pub timelines: Vec<String>,
}

#[derive(Debug)]
pub struct RunOutcome {
    pub out_dir: PathBuf,
    pub decisions_path: PathBuf,
    pub decisions_sha256: String,
    pub deployment: String,
    pub block: u64,
    pub block_timestamp: i64,
    pub head_number: u64,
    pub head_timestamp: i64,
    pub rows: Vec<(String, Decision, Option<String>)>,
    pub header: Header,
    /// The typed records as written, for callers that anchor or test them
    /// (06-arc-hook.md reads `record_sha256` from here).
    #[allow(dead_code)]
    pub records: Vec<Record>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Header {
    pub decided: usize,
    pub allow: usize,
    pub review: usize,
    pub unindexed: Vec<String>,
    pub wrappers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedIdent {
    pub address: String,
    pub product: String,
    pub issuer: String,
    pub family: Family,
    pub registry_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub subgraph: SubgraphEvidence,
    pub crossfoot: Option<CrossfootEvidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockRef {
    pub number: u64,
    pub hash: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubgraphProvenance {
    pub endpoint: Option<String>,
    pub source: &'static str,
    pub deployment: String,
    pub deployment_digest: Option<String>,
    /// The block every query except Head was pinned to.
    pub block: BlockRef,
    /// The live indexed head at run time, from the Head query.
    pub head: BlockRef,
    pub has_indexing_errors: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryProvenance {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument: Option<String>,
    pub query_sha256: String,
    pub variables_sha256: String,
    pub response_sha256: String,
    pub response_file: String,
}

/// The policy file as the record carries it: name, hash and gates, so a
/// reader can re-check every POLICY_ word from the record alone.
#[derive(Debug, Clone, Serialize)]
pub struct EligibilityProvenance {
    pub file: String,
    pub name: String,
    pub sha256: String,
    pub gates: crate::model::decision::PolicyGates,
}

#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    pub subgraph: SubgraphProvenance,
    pub queries: Vec<QueryProvenance>,
    pub feeds_json_sha256: String,
    pub now_unix: i64,
    pub policy: Policy,
    pub eligibility: Option<EligibilityProvenance>,
}

pub const DEFAULT_POLICY_PATH: &str = "config/policy-default.json";

/// Reads the policy file: `--policy` when given, else the default path when
/// it exists, else none.
fn load_policy(
    path: Option<&Path>,
) -> Result<Option<(EligibilityPolicy, EligibilityProvenance)>, String> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => {
            let default = PathBuf::from(DEFAULT_POLICY_PATH);
            if !default.exists() {
                return Ok(None);
            }
            default
        }
    };
    let bytes = fs::read(&path)
        .map_err(|err| format!("policy {} is not readable: {err}", path.display()))?;
    let policy: EligibilityPolicy = serde_json::from_slice(&bytes)
        .map_err(|err| format!("policy {} does not parse: {err}", path.display()))?;
    if policy.format != crate::model::decision::POLICY_FORMAT {
        return Err(format!(
            "policy {} has format {}, expected {}",
            path.display(),
            policy.format,
            crate::model::decision::POLICY_FORMAT
        ));
    }
    let provenance = EligibilityProvenance {
        file: path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string()),
        name: policy.name.clone(),
        sha256: sha256_hex(&bytes),
        gates: policy.gates.clone(),
    };
    Ok(Some((policy, provenance)))
}

#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub tool_version: String,
    pub git_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub feed: FeedIdent,
    pub decision: Decision,
    pub reason: Option<String>,
    pub reasons: Vec<String>,
    pub reason_text: String,
    pub notes: Vec<String>,
    pub evidence: Evidence,
    pub provenance: Provenance,
    pub agent: Agent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Output {
    pub format: &'static str,
    pub header: Header,
    pub decisions: Vec<Record>,
}

/// One executed query: what was sent, what came back, and the hashes.
struct Executed {
    name: String,
    argument: Option<String>,
    file: String,
    query_sha256: String,
    variables_sha256: String,
    body: String,
}

impl Executed {
    fn provenance(&self) -> QueryProvenance {
        QueryProvenance {
            name: self.name.clone(),
            argument: self.argument.clone(),
            query_sha256: self.query_sha256.clone(),
            variables_sha256: self.variables_sha256.clone(),
            response_sha256: sha256_hex(self.body.as_bytes()),
            response_file: format!("responses/{}", self.file),
        }
    }
}

enum Source {
    Network {
        endpoint: String,
        key: Option<String>,
        agent: ureq::Agent,
    },
    Replay(PathBuf),
}

impl Source {
    fn label(&self) -> &'static str {
        match self {
            Source::Network { .. } => "network",
            Source::Replay(_) => "replay",
        }
    }

    fn execute(
        &self,
        name: &str,
        argument: Option<&str>,
        query: &str,
        variables: &Value,
    ) -> Result<Executed, String> {
        let file = match argument {
            Some(arg) => format!("{name}-{}.json", crate::util::slug(arg)),
            None => format!("{name}.json"),
        };
        let variables_json = canonical_json(variables);
        let body = match self {
            Source::Replay(dir) => {
                let path = dir.join("responses").join(&file);
                fs::read_to_string(&path).map_err(|err| {
                    format!("replay response {} is not readable: {err}", path.display())
                })?
            }
            Source::Network {
                endpoint,
                key,
                agent,
            } => {
                let request = json!({"query": query, "variables": variables});
                let request = serde_json::to_string(&request).expect("request serialises");
                let redacted = redact_endpoint(endpoint);
                let mut call = agent
                    .post(endpoint)
                    .set("content-type", "application/json")
                    .set("accept", "application/json");
                if let Some(key) = key {
                    call = call.set("authorization", &format!("Bearer {key}"));
                }
                match call.send_string(&request) {
                    Ok(response) => response.into_string().map_err(|err| {
                        format!("{name}: could not read the response body from {redacted}: {err}")
                    })?,
                    Err(ureq::Error::Status(status, response)) => {
                        let text = response.into_string().unwrap_or_default();
                        return Err(format!(
                            "{name}: HTTP {status} from {redacted}: {}",
                            truncate(&text, 300)
                        ));
                    }
                    Err(ureq::Error::Transport(transport)) => {
                        // The transport message can carry the URL, and with it
                        // a key in the path; only the kind is reported.
                        return Err(format!(
                            "{name}: endpoint {redacted} did not answer ({:?})",
                            transport.kind()
                        ));
                    }
                }
            }
        };
        Ok(Executed {
            name: name.to_string(),
            argument: argument.map(str::to_string),
            file,
            query_sha256: sha256_hex(query.as_bytes()),
            variables_sha256: sha256_hex(variables_json.as_bytes()),
            body,
        })
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    }
}

/// Compact JSON with keys in sorted order (serde_json's map is ordered), so
/// the same variable set always hashes the same.
pub fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON value serialises")
}

/// The 32 byte digest behind a `Qm...` deployment ID: base58 decoded, the
/// multihash prefix `0x1220` removed, as hex (05 R13). None when the ID is
/// not a base58 sha256 multihash.
pub fn deployment_digest(id: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut bytes: Vec<u8> = Vec::new();
    for c in id.bytes() {
        let mut carry = ALPHABET.iter().position(|&a| a == c)? as u32;
        for b in bytes.iter_mut().rev() {
            let v = (*b as u32) * 58 + carry;
            *b = (v & 0xff) as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            bytes.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let zeros = id.bytes().take_while(|&c| c == b'1').count();
    let mut out = vec![0u8; zeros];
    out.extend(bytes);
    if out.len() == 34 && out[0] == 0x12 && out[1] == 0x20 {
        Some(hex_encode(&out[2..]))
    } else {
        None
    }
}

fn read_query(dir: &Path, name: &str) -> Result<String, String> {
    let path = dir.join(format!("{name}.graphql"));
    fs::read_to_string(&path)
        .map_err(|err| format!("query file {} is not readable: {err}", path.display()))
}

/// The GraphQL `data` object of a response body; a body with `errors` or
/// without `data` is a failed query (05 R15).
fn response_data(name: &str, body: &str) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(body).map_err(|err| format!("{name}: response is not JSON: {err}"))?;
    if let Some(errors) = parsed.get("errors").and_then(Value::as_array) {
        if let Some(first) = errors.first() {
            let message = first
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!(
                "{name}: the subgraph answered with an error: {message}"
            ));
        }
    }
    match parsed.get("data") {
        Some(data) if data.is_object() => Ok(data.clone()),
        _ => Err(format!("{name}: response carries no data object")),
    }
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn int_field(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn require_int(name: &str, value: &Value, key: &str) -> Result<i64, String> {
    int_field(value, key).ok_or_else(|| format!("{name}: field {key} is missing or not an integer"))
}

fn require_str(name: &str, value: &Value, key: &str) -> Result<String, String> {
    str_field(value, key).ok_or_else(|| format!("{name}: field {key} is missing or not a string"))
}

fn meta_block_number(name: &str, data: &Value) -> Result<u64, String> {
    let block = data
        .get("_meta")
        .and_then(|m| m.get("block"))
        .ok_or_else(|| format!("{name}: _meta.block is missing"))?;
    Ok(require_int(name, block, "number")? as u64)
}

fn meta_deployment(data: &Value) -> Option<String> {
    data.get("_meta").and_then(|m| str_field(m, "deployment"))
}

fn meta_block_hash(data: &Value) -> Option<String> {
    data.get("_meta")
        .and_then(|m| m.get("block"))
        .and_then(|b| str_field(b, "hash"))
}

/// `_meta` with deployment, number, timestamp and the error flag, as the
/// Head and FeedStatus queries select it.
fn parse_meta(name: &str, data: &Value) -> Result<Head, String> {
    let meta = data
        .get("_meta")
        .ok_or_else(|| format!("{name}: _meta is missing"))?;
    let block = meta
        .get("block")
        .ok_or_else(|| format!("{name}: _meta.block is missing"))?;
    Ok(Head {
        deployment: require_str(name, meta, "deployment")?,
        number: require_int(name, block, "number")? as u64,
        timestamp: require_int(name, block, "timestamp")?,
        has_indexing_errors: meta
            .get("hasIndexingErrors")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The note on every record when the pinned block's timestamp had to be
/// taken from the head: graph-node answers `_meta(block: {number})` with a
/// null hash and timestamp, so a run pinned below the head measures feed
/// and result freshness against the head's timestamp.
pub const PINNED_TIMESTAMP_NOTE: &str = "the subgraph returned no timestamp for the pinned block; feed and result freshness were measured against the head timestamp";

/// `_meta` of a pinned query: deployment, number and the timestamp when
/// the node returned one (Studio returns null for a pinned block).
fn parse_pinned_meta(name: &str, data: &Value) -> Result<(String, u64, Option<i64>), String> {
    let meta = data
        .get("_meta")
        .ok_or_else(|| format!("{name}: _meta is missing"))?;
    let block = meta
        .get("block")
        .ok_or_else(|| format!("{name}: _meta.block is missing"))?;
    Ok((
        require_str(name, meta, "deployment")?,
        require_int(name, block, "number")? as u64,
        int_field(block, "timestamp"),
    ))
}

fn parse_feeds(data: &Value) -> Result<Vec<SubgraphFeed>, String> {
    let list = data
        .get("feeds")
        .and_then(Value::as_array)
        .ok_or_else(|| "FeedStatus: feeds is missing".to_string())?;
    let mut out = Vec::with_capacity(list.len());
    for feed in list {
        let address = require_str("FeedStatus", feed, "id")?.to_ascii_lowercase();
        let family = match feed.get("family").and_then(Value::as_str) {
            Some("POSTED") => Family::Posted,
            Some("DERIVED") => Family::Derived,
            other => {
                return Err(format!(
                    "FeedStatus: feed {address} has family {other:?}, expected POSTED or DERIVED"
                ))
            }
        };
        let latest_round = feed
            .get("latestRound")
            .filter(|v| v.is_object())
            .map(|round| LatestRound {
                round_id: str_field(round, "roundId").unwrap_or_default(),
                path: str_field(round, "path").unwrap_or_default(),
                over_bound: round
                    .get("overBound")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                updated_at: str_field(round, "updatedAt"),
            });
        out.push(SubgraphFeed {
            address,
            family,
            issuer: str_field(feed, "issuer").unwrap_or_default(),
            product: str_field(feed, "product").unwrap_or_default(),
            registry_key: str_field(feed, "registryKey"),
            bound: str_field(feed, "bound"),
            latest_answer: str_field(feed, "latestAnswer"),
            latest_updated_at: int_field(feed, "latestUpdatedAt"),
            round_count: int_field(feed, "roundCount").unwrap_or(0),
            unchecked_count: int_field(feed, "uncheckedCount").unwrap_or(0),
            over_bound_count: int_field(feed, "overBoundCount").unwrap_or(0),
            latest_round,
            recent_answers: feed
                .get("rounds")
                .and_then(Value::as_array)
                .map(|rounds| {
                    rounds
                        .iter()
                        .filter_map(|r| {
                            Some((str_field(r, "answer")?, int_field(r, "blockTimestamp")?))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| a.address.cmp(&b.address));
    Ok(out)
}

fn feed_id_of(row: &Value) -> String {
    row.get("feed")
        .and_then(|f| str_field(f, "id"))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

struct WindowFindings {
    unchecked: BTreeMap<String, Vec<UncheckedRound>>,
    unknown: BTreeMap<String, Vec<UnknownRound>>,
    bound_changes: BTreeMap<String, Vec<BoundChangeRow>>,
    rate_changes: BTreeMap<String, Vec<RateChangeRow>>,
}

fn parse_window(data: &Value) -> Result<WindowFindings, String> {
    let list = |key: &str| -> Result<&Vec<Value>, String> {
        data.get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("WindowFindings: {key} is missing"))
    };
    let mut findings = WindowFindings {
        unchecked: BTreeMap::new(),
        unknown: BTreeMap::new(),
        bound_changes: BTreeMap::new(),
        rate_changes: BTreeMap::new(),
    };
    for round in list("overBound")? {
        findings
            .unchecked
            .entry(feed_id_of(round))
            .or_default()
            .push(UncheckedRound {
                round_id: str_field(round, "roundId").unwrap_or_default(),
                block: require_int("WindowFindings.overBound", round, "block")? as u64,
                block_timestamp: int_field(round, "blockTimestamp"),
                tx: str_field(round, "tx").unwrap_or_default(),
                selector: str_field(round, "selector"),
                poster: str_field(round, "poster"),
                answer: str_field(round, "answer").unwrap_or_default(),
                previous_answer: str_field(round, "previousAnswer"),
                deviation: str_field(round, "deviationFromPrevious"),
                bound_at_post: str_field(round, "boundAtPost"),
                over_bound: round
                    .get("overBound")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
    }
    for round in list("unknown")? {
        findings
            .unknown
            .entry(feed_id_of(round))
            .or_default()
            .push(UnknownRound {
                round_id: str_field(round, "roundId").unwrap_or_default(),
                block: require_int("WindowFindings.unknown", round, "block")? as u64,
                tx: str_field(round, "tx").unwrap_or_default(),
            });
    }
    for change in list("boundChanges")? {
        findings
            .bound_changes
            .entry(feed_id_of(change))
            .or_default()
            .push(BoundChangeRow {
                old_bound: str_field(change, "oldBound"),
                new_bound: str_field(change, "newBound"),
                old_min_answer: str_field(change, "oldMinAnswer"),
                new_min_answer: str_field(change, "newMinAnswer"),
                old_max_answer: str_field(change, "oldMaxAnswer"),
                new_max_answer: str_field(change, "newMaxAnswer"),
                block: require_int("WindowFindings.boundChanges", change, "block")? as u64,
                tx: str_field(change, "tx").unwrap_or_default(),
                caller: str_field(change, "caller").unwrap_or_default(),
            });
    }
    for change in list("rateChanges")? {
        findings
            .rate_changes
            .entry(feed_id_of(change))
            .or_default()
            .push(RateChangeRow {
                rate_ppm: int_field(change, "ratePPM").unwrap_or(0),
                block: require_int("WindowFindings.rateChanges", change, "block")? as u64,
                tx: str_field(change, "tx").unwrap_or_default(),
            });
    }
    // Deterministic order inside each feed, whatever the endpoint returned.
    for rounds in findings.unchecked.values_mut() {
        rounds.sort_by(|a, b| (a.block, &a.round_id).cmp(&(b.block, &b.round_id)));
    }
    for rounds in findings.unknown.values_mut() {
        rounds.sort_by(|a, b| (a.block, &a.round_id).cmp(&(b.block, &b.round_id)));
    }
    for changes in findings.bound_changes.values_mut() {
        changes.sort_by(|a, b| (a.block, &a.tx).cmp(&(b.block, &b.tx)));
    }
    for changes in findings.rate_changes.values_mut() {
        changes.sort_by(|a, b| (a.block, &a.tx).cmp(&(b.block, &b.tx)));
    }
    Ok(findings)
}

fn parse_timeline(data: &Value) -> Result<Option<(String, TimelineEvidence)>, String> {
    let Some(feed) = data.get("feed").filter(|f| f.is_object()) else {
        return Ok(None);
    };
    let address = require_str("FeedTimeline", feed, "id")?.to_ascii_lowercase();
    let rounds = feed
        .get("rounds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut unchecked_round_ids = Vec::new();
    let mut over_bound_round_ids = Vec::new();
    for round in &rounds {
        let id = str_field(round, "roundId").unwrap_or_default();
        if round.get("path").and_then(Value::as_str) == Some("UNCHECKED") {
            unchecked_round_ids.push(id.clone());
        }
        if round.get("overBound").and_then(Value::as_bool) == Some(true) {
            over_bound_round_ids.push(id);
        }
    }
    Ok(Some((
        address,
        TimelineEvidence {
            round_count: int_field(feed, "roundCount").unwrap_or(0),
            rounds_returned: rounds.len(),
            unchecked_round_ids,
            over_bound_round_ids,
            bound_changes: feed
                .get("boundChanges")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
        },
    )))
}

/// `feeds.json` as a bare array or an object holding `rows` (or `feeds`).
pub fn load_feed_rows(path: &Path) -> Result<(Vec<CrossfootRow>, String), String> {
    let bytes = fs::read(path)
        .map_err(|err| format!("feeds.json {} is not readable: {err}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("feeds.json {} is not JSON: {err}", path.display()))?;
    let list = match &value {
        Value::Array(rows) => rows.clone(),
        Value::Object(map) => map
            .get("rows")
            .or_else(|| map.get("feeds"))
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "feeds.json {} holds neither an array nor a rows list",
                    path.display()
                )
            })?,
        _ => return Err(format!("feeds.json {} is not a list", path.display())),
    };
    let mut rows = Vec::with_capacity(list.len());
    for (index, row) in list.into_iter().enumerate() {
        let row: CrossfootRow = serde_json::from_value(row)
            .map_err(|err| format!("feeds.json row {index} does not parse: {err}"))?;
        rows.push(row);
    }
    Ok((rows, sha256_hex(&bytes)))
}

/// 05 R3: per lowercase address the row with the greatest block, ties to the
/// `midas` target, then the target name so the choice is deterministic.
pub fn join_rows(rows: &[CrossfootRow]) -> BTreeMap<String, CrossfootRow> {
    let mut best: BTreeMap<String, CrossfootRow> = BTreeMap::new();
    for row in rows {
        let key = row.address.to_ascii_lowercase();
        let replace = match best.get(&key) {
            None => true,
            Some(current) => {
                let rank = |r: &CrossfootRow| {
                    (
                        r.block,
                        r.target == "midas",
                        std::cmp::Reverse(r.target.clone()),
                    )
                };
                rank(row) > rank(current)
            }
        };
        if replace {
            best.insert(key, row.clone());
        }
    }
    best
}

/// Addresses of the entries with `kind: "derived"` in the Midas feed list,
/// lowercase and sorted. A missing file gives an empty list.
pub fn load_wrappers(path: &Path) -> Result<Vec<String>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("{} is not readable: {err}", path.display())),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("{} is not JSON: {err}", path.display()))?;
    let mut out: BTreeSet<String> = BTreeSet::new();
    if let Some(feeds) = value.get("feeds").and_then(Value::as_array) {
        for feed in feeds {
            if feed.get("kind").and_then(Value::as_str) == Some("derived") {
                if let Some(address) = feed.get("address").and_then(Value::as_str) {
                    out.insert(address.to_ascii_lowercase());
                }
            }
        }
    }
    Ok(out.into_iter().collect())
}

fn stamp_for(now: Option<i64>) -> String {
    match now.and_then(|n| chrono::DateTime::from_timestamp(n, 0)) {
        Some(dt) => dt.format("%Y%m%dT%H%M%SZ").to_string(),
        None => now_stamp(),
    }
}

fn create_run_dir(out: &Path, stamp: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(out).map_err(|err| format!("cannot create {}: {err}", out.display()))?;
    let mut candidate = out.join(stamp);
    let mut suffix = 2;
    while candidate.exists() {
        candidate = out.join(format!("{stamp}-{suffix}"));
        suffix += 1;
    }
    fs::create_dir(&candidate)
        .map_err(|err| format!("cannot create {}: {err}", candidate.display()))?;
    fs::create_dir(candidate.join("responses"))
        .map_err(|err| format!("cannot create {}/responses: {err}", candidate.display()))?;
    Ok(candidate)
}

/// The record's canonical JSON is its compact serialisation without the
/// `record_sha256` key (05 R13).
pub fn record_sha256(record: &Record) -> String {
    let mut clone = record.clone();
    clone.record_sha256 = None;
    sha256_hex(
        serde_json::to_string(&clone)
            .expect("record serialises")
            .as_bytes(),
    )
}

pub fn run(opts: &ConsumeOpts) -> Result<RunOutcome, String> {
    let key = std::env::var(ENV_SUBGRAPH_KEY)
        .ok()
        .filter(|k| !k.is_empty());
    run_with_key(opts, key)
}

pub fn run_with_key(opts: &ConsumeOpts, key: Option<String>) -> Result<RunOutcome, String> {
    let policy = Policy {
        window_days: opts.window_days,
        stale_after_days: opts.stale_after_days,
        max_head_lag_seconds: opts.max_head_lag_seconds,
        max_result_age_days: opts.max_result_age_days,
    };
    let now = opts.now.unwrap_or_else(|| chrono::Utc::now().timestamp());

    let endpoint = opts
        .subgraph
        .clone()
        .or_else(|| std::env::var(ENV_SUBGRAPH_URL).ok())
        .filter(|e| !e.is_empty());
    let source = match &opts.replay {
        Some(dir) => Source::Replay(dir.clone()),
        None => Source::Network {
            endpoint: endpoint.clone().ok_or_else(|| {
                format!("no subgraph endpoint: pass --subgraph or set {ENV_SUBGRAPH_URL}")
            })?,
            key,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(60))
                .build(),
        },
    };

    // Inputs that do not need the network first, so a missing feeds.json
    // fails before any query is sent.
    let (rows, feeds_json_sha256) = load_feed_rows(&opts.feeds)?;
    let eligibility = load_policy(opts.policy.as_deref())?;
    let joined = join_rows(&rows);
    let wrappers = load_wrappers(&opts.midas_config)?;
    let queries: BTreeMap<&str, String> = QUERY_NAMES
        .iter()
        .map(|name| read_query(&opts.queries, name).map(|text| (*name, text)))
        .collect::<Result<_, _>>()?;

    let mut executed: Vec<Executed> = Vec::new();

    // Head first, on every run: the live indexed head and the error flag.
    let head_exec = source.execute("Head", None, &queries["Head"], &json!({}))?;
    let head_data = response_data("Head", &head_exec.body)?;
    let head = parse_meta("Head", &head_data)?;
    let head_hash = meta_block_hash(&head_data);
    executed.push(head_exec);

    // The pinned block: given, else the head; on replay without --block the
    // block the recorded FeedStatus was pinned to, so a directory recorded
    // with --block replays without repeating the number.
    let block = match (opts.block, &source) {
        (Some(block), _) => block,
        (None, Source::Network { .. }) => head.number,
        (None, Source::Replay(dir)) => {
            let status_file = dir.join("responses").join("FeedStatus.json");
            let body = fs::read_to_string(&status_file).map_err(|err| {
                format!(
                    "replay response {} is not readable: {err}",
                    status_file.display()
                )
            })?;
            meta_block_number("FeedStatus", &response_data("FeedStatus", &body)?)?
        }
    };

    // FeedStatus at the pinned block.
    let status = source.execute(
        "FeedStatus",
        None,
        &queries["FeedStatus"],
        &json!({"block": block}),
    )?;
    let status_data = response_data("FeedStatus", &status.body)?;
    let (status_deployment, status_number, status_timestamp) =
        parse_pinned_meta("FeedStatus", &status_data)?;
    if status_number != block {
        return Err(format!(
            "FeedStatus: response is at block {status_number} but the run is pinned to {block}"
        ));
    }
    if status_deployment != head.deployment {
        return Err(format!(
            "FeedStatus: response comes from deployment {status_deployment}, Head from {}",
            head.deployment
        ));
    }
    let mut run_notes: Vec<String> = Vec::new();
    let pinned_timestamp = match status_timestamp {
        Some(timestamp) => timestamp,
        None if status_number == head.number => head.timestamp,
        None => {
            run_notes.push(PINNED_TIMESTAMP_NOTE.to_string());
            head.timestamp
        }
    };
    let pinned = Pinned {
        number: status_number,
        timestamp: pinned_timestamp,
    };
    let feeds = parse_feeds(&status_data)?;
    executed.push(status);

    // WindowFindings: $since from now and the window, $resultBlock from the
    // DERIVED row (the lowest block if several; the head when none).
    let since = now - (policy.window_days as i64) * 86_400;
    let result_block = feeds
        .iter()
        .filter(|f| f.family == Family::Derived)
        .filter_map(|f| joined.get(&f.address).map(|r| r.block))
        .min()
        .unwrap_or(block);
    let window = source.execute(
        "WindowFindings",
        None,
        &queries["WindowFindings"],
        &json!({
            "block": block,
            "since": since.to_string(),
            "resultBlock": result_block.to_string(),
        }),
    )?;
    let window_data = response_data("WindowFindings", &window.body)?;
    check_meta("WindowFindings", &window_data, &head.deployment, block)?;
    let findings = parse_window(&window_data)?;
    executed.push(window);

    // FeedTimeline, once per --timeline product or address.
    let mut timelines: BTreeMap<String, TimelineEvidence> = BTreeMap::new();
    for wanted in &opts.timelines {
        let address = if wanted.starts_with("0x") {
            wanted.to_ascii_lowercase()
        } else {
            feeds
                .iter()
                .find(|f| f.product == *wanted)
                .map(|f| f.address.clone())
                .ok_or_else(|| format!("--timeline {wanted}: no subgraph feed has that product"))?
        };
        let timeline = source.execute(
            "FeedTimeline",
            Some(wanted),
            &queries["FeedTimeline"],
            &json!({"block": block, "feed": address}),
        )?;
        let timeline_data = response_data("FeedTimeline", &timeline.body)?;
        check_meta("FeedTimeline", &timeline_data, &head.deployment, block)?;
        if let Some((feed_address, evidence)) = parse_timeline(&timeline_data)? {
            timelines.insert(feed_address, evidence);
        }
        executed.push(timeline);
    }

    // Provenance, shared by every record.
    let provenance = Provenance {
        subgraph: SubgraphProvenance {
            endpoint: endpoint.as_deref().map(redact_endpoint),
            source: source.label(),
            deployment: head.deployment.clone(),
            deployment_digest: deployment_digest(&head.deployment),
            block: BlockRef {
                number: pinned.number,
                hash: meta_block_hash(&status_data),
                timestamp: pinned.timestamp,
            },
            head: BlockRef {
                number: head.number,
                hash: head_hash,
                timestamp: head.timestamp,
            },
            has_indexing_errors: head.has_indexing_errors,
        },
        queries: executed.iter().map(Executed::provenance).collect(),
        feeds_json_sha256,
        now_unix: now,
        policy: policy.clone(),
        eligibility: eligibility.as_ref().map(|(_, p)| p.clone()),
    };
    let agent = Agent {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: std::env::current_dir()
            .ok()
            .and_then(|dir| git_provenance(&dir).commit),
    };

    // Decisions, feeds sorted by address.
    let mut records: Vec<Record> = Vec::with_capacity(feeds.len());
    let mut rows_out = Vec::with_capacity(feeds.len());
    let mut allow = 0usize;
    let mut review = 0usize;
    for feed in &feeds {
        let row = joined.get(&feed.address);
        let inputs = FeedInputs {
            head: &head,
            pinned,
            now,
            policy: &policy,
            feed,
            row,
            unchecked_rounds: findings
                .unchecked
                .get(&feed.address)
                .cloned()
                .unwrap_or_default(),
            unknown_rounds: findings
                .unknown
                .get(&feed.address)
                .cloned()
                .unwrap_or_default(),
            bound_changes: findings
                .bound_changes
                .get(&feed.address)
                .cloned()
                .unwrap_or_default(),
            rate_changes: findings
                .rate_changes
                .get(&feed.address)
                .cloned()
                .unwrap_or_default(),
            eligibility: eligibility.as_ref().map(|(p, _)| p),
        };
        let mut outcome = decide(&inputs);
        outcome.notes.extend(run_notes.iter().cloned());
        outcome.evidence.timeline = timelines.get(&feed.address).cloned();
        match outcome.decision {
            Decision::Allow => allow += 1,
            Decision::Review => review += 1,
        }
        let label = match &feed.registry_key {
            Some(key) => format!("{}.{key}", feed.product),
            None => feed.product.clone(),
        };
        rows_out.push((label, outcome.decision, outcome.reason.clone()));
        let mut record = Record {
            feed: FeedIdent {
                address: feed.address.clone(),
                product: feed.product.clone(),
                issuer: feed.issuer.clone(),
                family: feed.family,
                registry_key: feed.registry_key.clone(),
            },
            decision: outcome.decision,
            reason: outcome.reason,
            reasons: outcome.reasons,
            reason_text: outcome.reason_text,
            notes: outcome.notes,
            evidence: Evidence {
                subgraph: outcome.evidence,
                crossfoot: row.map(CrossfootEvidence::from_row),
            },
            provenance: provenance.clone(),
            agent: agent.clone(),
            record_sha256: None,
        };
        record.record_sha256 = Some(record_sha256(&record));
        records.push(record);
    }

    let indexed: BTreeSet<&str> = feeds.iter().map(|f| f.address.as_str()).collect();
    let wrapper_set: BTreeSet<&str> = wrappers.iter().map(String::as_str).collect();
    let unindexed: Vec<String> = joined
        .keys()
        .filter(|address| {
            !indexed.contains(address.as_str()) && !wrapper_set.contains(address.as_str())
        })
        .cloned()
        .collect();
    let header = Header {
        decided: records.len(),
        allow,
        review,
        unindexed,
        wrappers,
    };
    let output = Output {
        format: FORMAT,
        header: header.clone(),
        decisions: records.clone(),
    };

    // Files.
    let out_dir = create_run_dir(&opts.out, &stamp_for(opts.now))?;
    for query in &executed {
        let path = out_dir.join("responses").join(&query.file);
        fs::write(&path, query.body.as_bytes())
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    }
    let mut text = serde_json::to_string_pretty(&output).expect("output serialises");
    text.push('\n');
    let decisions_path = out_dir.join("decisions.json");
    fs::write(&decisions_path, text.as_bytes())
        .map_err(|err| format!("cannot write {}: {err}", decisions_path.display()))?;
    let decisions_sha256 = sha256_hex(text.as_bytes());
    fs::write(
        out_dir.join("decisions.sha256"),
        format!("{decisions_sha256}  decisions.json\n"),
    )
    .map_err(|err| format!("cannot write decisions.sha256: {err}"))?;

    Ok(RunOutcome {
        out_dir,
        decisions_path,
        decisions_sha256,
        deployment: head.deployment,
        block: pinned.number,
        block_timestamp: pinned.timestamp,
        head_number: head.number,
        head_timestamp: head.timestamp,
        rows: rows_out,
        header,
        records,
    })
}

/// Every response of a run must come from the same deployment at the same
/// block, or the record's provenance would be a lie.
fn check_meta(
    name: &str,
    data: &Value,
    expected_deployment: &str,
    block: u64,
) -> Result<(), String> {
    if data.get("_meta").is_none() {
        return Ok(());
    }
    let number = meta_block_number(name, data)?;
    if number != block {
        return Err(format!(
            "{name}: response is at block {number}, the run is pinned to {block}"
        ));
    }
    if let Some(deployment) = meta_deployment(data) {
        if deployment != expected_deployment {
            return Err(format!(
                "{name}: response comes from deployment {deployment}, Head from {expected_deployment}"
            ));
        }
    }
    Ok(())
}

pub fn print_outcome(outcome: &RunOutcome) {
    println!("deployment  {}", outcome.deployment);
    println!(
        "head  {} ({})",
        outcome.head_number,
        unix_to_utc(outcome.head_timestamp).unwrap_or_else(|| "invalid timestamp".into())
    );
    println!(
        "block  {} ({})",
        outcome.block,
        unix_to_utc(outcome.block_timestamp).unwrap_or_else(|| "invalid timestamp".into())
    );
    for (label, decision, reason) in &outcome.rows {
        println!(
            "{label}  {}  {}",
            decision.as_str(),
            reason.as_deref().unwrap_or("-")
        );
    }
    println!(
        "summary  decided {}, allow {}, review {}, unindexed {}, wrappers {}",
        outcome.header.decided,
        outcome.header.allow,
        outcome.header.review,
        outcome.header.unindexed.len(),
        outcome.header.wrappers.len()
    );
    println!("run  {}", outcome.out_dir.display());
    println!("decisions  {}", outcome.decisions_path.display());
    println!("sha256  {}", outcome.decisions_sha256);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf()
    }

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/consume-fixture-v1")
    }

    const FIXTURE_NOW: i64 = 1_788_289_368;
    const FIXTURE_BLOCK: u64 = 25_884_405;
    const FIXTURE_DEPLOYMENT: &str = "QmRaeyYsGxJcxVXnAvGEBbvFpSEZkJCa9rUM5dAemwWaxD";
    const FIXTURE_DIGEST: &str = "30297e40e0d0726b2eb69a623a5a088d81227dbf2a7223ba3b732d091709b74e";
    const MRE7: &str = "0x0a2a51f2f206447de3e3a80fcf92240244722395";
    const SVZCHF: &str = "0xe5f130253ff137f9917c0107659a4c5262abf6b0";

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("crossfoot-consume-{tag}-{}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_opts(out: &Path) -> ConsumeOpts {
        let fixture = fixture_dir();
        ConsumeOpts {
            subgraph: None,
            feeds: fixture.join("feeds.json"),
            queries: repo_root().join("subgraph/queries"),
            midas_config: fixture.join("midas-mainnet.json"),
            out: out.to_path_buf(),
            window_days: 183,
            stale_after_days: 30,
            max_head_lag_seconds: 900,
            max_result_age_days: 30,
            now: Some(FIXTURE_NOW),
            block: None,
            replay: Some(fixture),
            policy: Some(repo_root().join(DEFAULT_POLICY_PATH)),
            timelines: vec!["mRE7".into()],
        }
    }

    /// The policy file's hash and gates travel in every record, and the
    /// default policy leaves the fixture's decisions unchanged.
    #[test]
    fn policy_hash_and_gates_are_in_every_record() {
        let outcome = run_with_key(&fixture_opts(&temp("policy")), None).unwrap();
        let output = read_output(&outcome);
        let bytes = fs::read(repo_root().join(DEFAULT_POLICY_PATH)).unwrap();
        for record in output["decisions"].as_array().unwrap() {
            let e = &record["provenance"]["eligibility"];
            assert_eq!(e["name"], "default");
            assert_eq!(e["sha256"], sha256_hex(&bytes));
            assert_eq!(e["gates"]["max_seconds_since_last_post"], 604_800);
            assert_eq!(e["gates"]["max_unchecked_deviation_percent"], "5");
        }
        assert_eq!(output["header"]["allow"], 11);
        // Without a policy the field is null and nothing else changes.
        let mut opts = fixture_opts(&temp("no-policy"));
        opts.policy = Some(PathBuf::from("/nonexistent/policy.json"));
        assert!(run_with_key(&opts, None).unwrap_err().contains("policy"));
    }

    fn read_output(outcome: &RunOutcome) -> Value {
        serde_json::from_str(&fs::read_to_string(&outcome.decisions_path).unwrap()).unwrap()
    }

    fn record_for<'a>(output: &'a Value, address: &str) -> &'a Value {
        output["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["feed"]["address"] == address)
            .unwrap_or_else(|| panic!("no record for {address}"))
    }

    /// A replay directory built from synthetic response values. The head is
    /// one POSTED feed (mRE7) and one DERIVED feed (svZCHF) unless the caller
    /// passes its own FeedStatus.
    struct Synthetic {
        dir: PathBuf,
    }

    impl Synthetic {
        fn new(tag: &str) -> Self {
            let dir = temp(tag);
            fs::create_dir_all(dir.join("responses")).unwrap();
            let s = Synthetic { dir };
            s.write("Head.json", &json!({"data": {"_meta": meta()}}));
            s.write("FeedStatus.json", &default_feed_status());
            s.write("WindowFindings.json", &empty_window());
            s.write("FeedTimeline-mre7.json", &json!({"data": {"_meta": {"deployment": FIXTURE_DEPLOYMENT, "block": {"number": FIXTURE_BLOCK}}, "feed": null}}));
            s.write_feeds(&default_rows());
            s
        }

        fn write(&self, file: &str, value: &Value) {
            fs::write(
                self.dir.join("responses").join(file),
                serde_json::to_string_pretty(value).unwrap(),
            )
            .unwrap();
        }

        fn write_feeds(&self, rows: &[Value]) {
            fs::write(
                self.dir.join("feeds.json"),
                serde_json::to_string_pretty(
                    &json!({"format": "crossfoot-feeds-v1", "rows": rows}),
                )
                .unwrap(),
            )
            .unwrap();
        }

        fn opts(&self) -> ConsumeOpts {
            let mut opts = fixture_opts(&self.dir.join("out"));
            opts.feeds = self.dir.join("feeds.json");
            opts.midas_config = self.dir.join("missing-config.json");
            opts.replay = Some(self.dir.clone());
            opts
        }
    }

    fn meta() -> Value {
        json!({
            "deployment": FIXTURE_DEPLOYMENT,
            "block": {"number": FIXTURE_BLOCK, "hash": "0xabc", "timestamp": FIXTURE_NOW},
            "hasIndexingErrors": false
        })
    }

    fn mre7_feed() -> Value {
        json!({
            "id": MRE7, "family": "POSTED", "issuer": "Midas", "product": "mRE7",
            "registryKey": "customFeed", "decimals": 8, "bound": "36000000",
            "minAnswer": "0", "maxAnswer": "10000000000000", "latestAnswer": "107833620",
            "latestUpdatedAt": (FIXTURE_NOW - 7000).to_string(), "roundCount": 56,
            "uncheckedCount": 1, "overBoundCount": 1,
            "latestRound": {"roundId": "56", "path": "SAFE", "overBound": false, "updatedAt": (FIXTURE_NOW - 7000).to_string()}
        })
    }

    fn svzchf_feed() -> Value {
        json!({
            "id": SVZCHF, "family": "DERIVED", "issuer": "Frankencoin", "product": "svZCHF",
            "registryKey": null, "decimals": 18, "bound": null, "minAnswer": null, "maxAnswer": null,
            "latestAnswer": "1021764268673581424", "latestUpdatedAt": (FIXTURE_NOW - 400000).to_string(),
            "roundCount": 144, "uncheckedCount": 0, "overBoundCount": 0,
            "latestRound": {"roundId": "144", "path": "PROTOCOL", "overBound": false, "updatedAt": (FIXTURE_NOW - 400000).to_string()}
        })
    }

    fn default_feed_status() -> Value {
        json!({"data": {"_meta": meta(), "feeds": [mre7_feed(), svzchf_feed()]}})
    }

    fn empty_window() -> Value {
        json!({"data": {
            "_meta": {"deployment": FIXTURE_DEPLOYMENT, "block": {"number": FIXTURE_BLOCK}},
            "overBound": [], "unknown": [], "boundChanges": [], "rateChanges": []
        }})
    }

    fn mre7_row(block: u64, target: &str) -> Value {
        json!({
            "address": "0x0a2a51f2f206447dE3E3a80FCf92240244722395", "target": target, "product": "mRE7",
            "family": "guarded-setter", "verdict": "CONSISTENT", "posting_path": "GUARDED",
            "liveness": "LIVE", "consumer_action": "ALLOW", "nav_recomputation": "INPUT_GAP",
            "headline": "56 rounds", "bundle_root": "aa".repeat(32),
            "result_path": format!("bundles/{target}-run-{block}/result.json"), "block": block
        })
    }

    fn svzchf_row() -> Value {
        json!({
            "address": SVZCHF, "target": "svzchf", "product": "svZCHF", "family": "recomputable-accrual",
            "verdict": "MODEL_MATCH", "posting_path": null, "liveness": null, "consumer_action": "ALLOW",
            "nav_recomputation": "FULL", "headline": "5 of 5 fields exact, residual 0",
            "bundle_root": "bb".repeat(32), "result_path": "bundles/svzchf-run/result.json", "block": 25853000
        })
    }

    fn default_rows() -> Vec<Value> {
        vec![mre7_row(FIXTURE_BLOCK, "midas"), svzchf_row()]
    }

    #[test]
    fn deployment_digest_round_trips_the_qm_hash() {
        assert_eq!(
            deployment_digest(FIXTURE_DEPLOYMENT).as_deref(),
            Some(FIXTURE_DIGEST)
        );
        assert_eq!(deployment_digest("not-base58!"), None);
        assert_eq!(deployment_digest("Qm"), None);
    }

    /// 05 R1, R2: the three queries run with the documented variables and
    /// the hashes identify the query files and the variable sets.
    #[test]
    fn consume_runs_the_three_queries_with_the_documented_variables() {
        let out = temp("three-queries");
        let outcome = run_with_key(&fixture_opts(&out), None).unwrap();
        let output = read_output(&outcome);
        let record = record_for(&output, MRE7);
        let queries = record["provenance"]["queries"].as_array().unwrap();
        let names: Vec<&str> = queries
            .iter()
            .map(|q| q["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["Head", "FeedStatus", "WindowFindings", "FeedTimeline"]
        );
        assert_eq!(queries[3]["argument"], "mRE7");
        assert_eq!(
            queries[3]["response_file"],
            "responses/FeedTimeline-mre7.json"
        );
        assert_eq!(queries[0]["variables_sha256"], sha256_hex(b"{}"));

        let query_dir = repo_root().join("subgraph/queries");
        for query in queries {
            let name = query["name"].as_str().unwrap();
            let file = fs::read(query_dir.join(format!("{name}.graphql"))).unwrap();
            assert_eq!(query["query_sha256"], sha256_hex(&file), "{name}");
        }
        let since = FIXTURE_NOW - 183 * 86_400;
        let expected_window = canonical_json(&json!({
            "block": FIXTURE_BLOCK, "since": since.to_string(), "resultBlock": "25853000"
        }));
        assert_eq!(
            queries[2]["variables_sha256"],
            sha256_hex(expected_window.as_bytes())
        );
        assert_eq!(
            queries[1]["variables_sha256"],
            sha256_hex(canonical_json(&json!({"block": FIXTURE_BLOCK})).as_bytes())
        );
        assert_eq!(
            queries[3]["variables_sha256"],
            sha256_hex(canonical_json(&json!({"block": FIXTURE_BLOCK, "feed": MRE7})).as_bytes())
        );
        let recorded = fs::read(outcome.out_dir.join("responses/WindowFindings.json")).unwrap();
        assert_eq!(queries[2]["response_sha256"], sha256_hex(&recorded));
        assert_eq!(
            recorded,
            fs::read(fixture_dir().join("responses/WindowFindings.json")).unwrap(),
            "responses are copied verbatim"
        );
    }

    /// 05 R3: two rows for one address, the greater block wins, ties to midas.
    #[test]
    fn join_prefers_the_latest_block_then_the_midas_target() {
        let rows: Vec<CrossfootRow> = [
            mre7_row(25_850_000, "mtbill"),
            mre7_row(25_884_405, "midas"),
            mre7_row(25_884_405, "mtbill"),
        ]
        .into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect();
        let joined = join_rows(&rows);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[MRE7].target, "midas");
        assert_eq!(joined[MRE7].block, 25_884_405);

        let rows: Vec<CrossfootRow> = [
            mre7_row(25_884_405, "midas"),
            mre7_row(25_900_000, "mtbill"),
        ]
        .into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect();
        assert_eq!(join_rows(&rows)[MRE7].target, "mtbill");

        // Through the whole run: the synthetic feeds.json holds both rows.
        let s = Synthetic::new("join");
        s.write_feeds(&[
            mre7_row(25_850_000, "mtbill"),
            mre7_row(FIXTURE_BLOCK, "midas"),
            svzchf_row(),
        ]);
        let outcome = run_with_key(&s.opts(), None).unwrap();
        let output = read_output(&outcome);
        assert_eq!(
            record_for(&output, MRE7)["evidence"]["crossfoot"]["target"],
            "midas"
        );
        assert_eq!(output["header"]["unindexed"], json!([]));
    }

    /// 05 R3: a feeds.json row without a subgraph feed is listed, not decided.
    #[test]
    fn unindexed_rows_are_listed_not_decided() {
        let s = Synthetic::new("unindexed");
        let mut extra = mre7_row(FIXTURE_BLOCK, "midas");
        extra["address"] = json!("0x00000000000000000000000000000000000000AA");
        s.write_feeds(&[mre7_row(FIXTURE_BLOCK, "midas"), svzchf_row(), extra]);
        let outcome = run_with_key(&s.opts(), None).unwrap();
        let output = read_output(&outcome);
        assert_eq!(output["header"]["decided"], 2);
        assert_eq!(
            output["header"]["unindexed"],
            json!(["0x00000000000000000000000000000000000000aa"])
        );
        assert_eq!(output["decisions"].as_array().unwrap().len(), 2);
    }

    /// 05 R4: now 901 seconds past the head routes every feed to REVIEW.
    #[test]
    fn stale_head_routes_every_feed_to_review() {
        let out = temp("stale-head");
        let mut opts = fixture_opts(&out);
        opts.now = Some(FIXTURE_NOW + 901);
        let outcome = run_with_key(&opts, None).unwrap();
        assert_eq!(outcome.header.decided, 61);
        assert_eq!(outcome.header.review, 61);
        assert_eq!(outcome.header.allow, 0);
        assert!(outcome
            .rows
            .iter()
            .all(|(_, d, r)| *d == Decision::Review && r.as_deref() == Some("SUBGRAPH_STALE")));
        // Exit code 0: every indexed feed received a decision.
        let output = read_output(&outcome);
        let svzchf = record_for(&output, SVZCHF);
        assert_eq!(svzchf["reasons"], json!(["SUBGRAPH_STALE"]));
    }

    /// A pinned `_meta` without hash and timestamp (what Studio returns for
    /// a block-pinned query): the head's timestamp serves, silently when
    /// the pinned block is the head and with the note when it is older.
    #[test]
    fn pinned_block_without_timestamp_uses_the_head_timestamp() {
        let s = Synthetic::new("pinned-no-timestamp");
        let mut status = default_feed_status();
        status["data"]["_meta"]["block"] = json!({"number": FIXTURE_BLOCK});
        s.write("FeedStatus.json", &status);
        let outcome = run_with_key(&s.opts(), None).unwrap();
        assert_eq!(outcome.block_timestamp, FIXTURE_NOW);
        let output = read_output(&outcome);
        let record = record_for(&output, MRE7);
        assert_eq!(
            record["provenance"]["subgraph"]["block"]["timestamp"],
            FIXTURE_NOW
        );
        assert_eq!(
            record["provenance"]["subgraph"]["block"]["hash"],
            Value::Null
        );
        assert!(record["notes"].as_array().unwrap().is_empty());

        // The head moved on: the pinned block is older, so the note is set.
        let mut head = meta();
        head["block"]["number"] = json!(FIXTURE_BLOCK + 500);
        head["block"]["timestamp"] = json!(FIXTURE_NOW + 6_000);
        s.write("Head.json", &json!({"data": {"_meta": head}}));
        let mut opts = s.opts();
        opts.now = Some(FIXTURE_NOW + 6_000);
        let outcome = run_with_key(&opts, None).unwrap();
        let output = read_output(&outcome);
        let record = record_for(&output, MRE7);
        assert_eq!(record["decision"], "ALLOW");
        assert_eq!(record["notes"], json!([PINNED_TIMESTAMP_NOTE]));
        assert_eq!(
            record["provenance"]["subgraph"]["head"]["number"],
            FIXTURE_BLOCK + 500
        );
        assert_eq!(
            record["provenance"]["subgraph"]["block"]["number"],
            FIXTURE_BLOCK
        );
    }

    /// 05 R4: hasIndexingErrors routes every feed to REVIEW.
    #[test]
    fn indexing_errors_route_every_feed_to_review() {
        let s = Synthetic::new("indexing-errors");
        let mut head = meta();
        head["hasIndexingErrors"] = json!(true);
        s.write("Head.json", &json!({"data": {"_meta": head}}));
        let outcome = run_with_key(&s.opts(), None).unwrap();
        assert_eq!(outcome.header.review, 2);
        assert!(outcome
            .rows
            .iter()
            .all(|(_, _, r)| r.as_deref() == Some("INDEXING_ERRORS")));
        let output = read_output(&outcome);
        assert_eq!(
            record_for(&output, MRE7)["provenance"]["subgraph"]["has_indexing_errors"],
            true
        );
    }

    /// 05 R7: string equality on the mRE7 round 36 sentence.
    #[test]
    fn reason_text_for_round_36_is_exact() {
        let out = temp("round-36");
        let outcome = run_with_key(&fixture_opts(&out), None).unwrap();
        let output = read_output(&outcome);
        let record = record_for(&output, MRE7);
        let root = record["evidence"]["crossfoot"]["bundle_root"]
            .as_str()
            .unwrap();
        assert_eq!(root.len(), 64);
        assert_eq!(
            record["reason_text"],
            format!(
                "ADMIN_GUARD_BYPASSED: round 36 posted through setRoundData (0xa4381d1f) at block 25037959, deviation 2.22466613 percent against bound 0.36 percent in force; tx 0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733; Crossfoot posting_path ADMIN_GUARD_BYPASSED, bundle {root}"
            )
        );
        assert_eq!(record["reason"], "ADMIN_GUARD_BYPASSED");
        assert_eq!(
            record["evidence"]["subgraph"]["over_bound_rounds"][0]["round_id"],
            "36"
        );
        assert_eq!(
            record["evidence"]["subgraph"]["over_bound_rounds"][0]["deviation"],
            "222466613"
        );
        assert_eq!(
            record["evidence"]["subgraph"]["over_bound_rounds"][0]["bound_at_post"],
            "36000000"
        );
        assert_eq!(
            record["evidence"]["crossfoot"]["posting_path"],
            "ADMIN_GUARD_BYPASSED"
        );
    }

    /// 05 R9, R13: every provenance field is present on every record.
    #[test]
    fn record_carries_every_provenance_field() {
        let out = temp("provenance");
        let outcome = run_with_key(&fixture_opts(&out), None).unwrap();
        let output = read_output(&outcome);
        assert_eq!(output["format"], FORMAT);
        for key in ["decided", "allow", "review", "unindexed", "wrappers"] {
            assert!(output["header"].get(key).is_some(), "header.{key}");
        }
        assert_eq!(output["header"]["wrappers"].as_array().unwrap().len(), 6);
        for record in output["decisions"].as_array().unwrap() {
            for key in [
                "feed",
                "decision",
                "reason",
                "reasons",
                "reason_text",
                "notes",
                "evidence",
                "provenance",
                "agent",
                "record_sha256",
            ] {
                assert!(record.get(key).is_some(), "record.{key}");
            }
            for key in ["address", "product", "issuer", "family", "registry_key"] {
                assert!(record["feed"].get(key).is_some(), "feed.{key}");
            }
            for key in ["subgraph", "crossfoot"] {
                assert!(record["evidence"].get(key).is_some(), "evidence.{key}");
            }
            for key in [
                "latest_round",
                "over_bound_rounds",
                "unknown_rounds",
                "bound_changes",
                "rate_changes_after_window",
            ] {
                assert!(
                    record["evidence"]["subgraph"].get(key).is_some(),
                    "evidence.subgraph.{key}"
                );
            }
            let p = &record["provenance"];
            for key in [
                "subgraph",
                "queries",
                "feeds_json_sha256",
                "now_unix",
                "policy",
            ] {
                assert!(p.get(key).is_some(), "provenance.{key}");
            }
            for key in [
                "endpoint",
                "head",
                "source",
                "deployment",
                "deployment_digest",
                "block",
                "has_indexing_errors",
            ] {
                assert!(
                    p["subgraph"].get(key).is_some(),
                    "provenance.subgraph.{key}"
                );
            }
            assert_eq!(p["subgraph"]["source"], "replay");
            assert_eq!(p["subgraph"]["deployment"], FIXTURE_DEPLOYMENT);
            assert_eq!(p["subgraph"]["deployment_digest"], FIXTURE_DIGEST);
            assert_eq!(p["subgraph"]["block"]["number"], FIXTURE_BLOCK);
            assert_eq!(p["subgraph"]["block"]["timestamp"], FIXTURE_NOW);
            assert_eq!(p["now_unix"], FIXTURE_NOW);
            for key in [
                "window_days",
                "stale_after_days",
                "max_head_lag_seconds",
                "max_result_age_days",
            ] {
                assert!(p["policy"].get(key).is_some(), "policy.{key}");
            }
            for query in p["queries"].as_array().unwrap() {
                for key in [
                    "name",
                    "query_sha256",
                    "variables_sha256",
                    "response_sha256",
                    "response_file",
                ] {
                    assert!(
                        query[key].as_str().map(str::len).unwrap_or(0) > 0,
                        "queries.{key}"
                    );
                }
            }
            assert_eq!(record["agent"]["tool_version"], env!("CARGO_PKG_VERSION"));
            assert!(record["agent"].get("git_commit").is_some());
            assert_eq!(record["record_sha256"].as_str().unwrap().len(), 64);
        }
    }

    /// 05 R13: record_sha256 hashes the record's canonical JSON without
    /// itself, and the stored value is what a reader recomputes.
    #[test]
    fn record_sha256_excludes_itself() {
        let outcome = run_with_key(&fixture_opts(&temp("record-sha")), None).unwrap();
        assert_eq!(outcome.records.len(), 61);
        let output = read_output(&outcome);
        for record in &outcome.records {
            let stored = record.record_sha256.clone().expect("hash set");
            assert_eq!(record_sha256(record), stored);
            let mut without = record.clone();
            without.record_sha256 = None;
            let canonical = serde_json::to_string(&without).unwrap();
            assert!(!canonical.contains("record_sha256"));
            assert_eq!(sha256_hex(canonical.as_bytes()), stored);
            let with = serde_json::to_string(record).unwrap();
            assert_ne!(sha256_hex(with.as_bytes()), stored);
            assert_eq!(
                record_for(&output, &record.feed.address)["record_sha256"],
                stored,
                "the file carries the same hash"
            );
        }
    }

    /// 05 R10: two runs from the same replay directory, feeds.json and
    /// --now write byte-identical decisions.json.
    #[test]
    fn consume_twice_from_replay_is_byte_identical() {
        let a = run_with_key(&fixture_opts(&temp("twice-a")), None).unwrap();
        let b = run_with_key(&fixture_opts(&temp("twice-b")), None).unwrap();
        assert_ne!(a.out_dir, b.out_dir);
        assert_eq!(
            fs::read(&a.decisions_path).unwrap(),
            fs::read(&b.decisions_path).unwrap()
        );
        assert_eq!(a.decisions_sha256, b.decisions_sha256);
        assert_eq!(
            fs::read_to_string(a.out_dir.join("decisions.sha256")).unwrap(),
            format!("{}  decisions.json\n", a.decisions_sha256)
        );
        let text = fs::read(&a.decisions_path).unwrap();
        assert_eq!(sha256_hex(&text), a.decisions_sha256);
    }

    /// 05 R10: with --replay the agent never opens a socket, whatever the
    /// endpoint says.
    #[test]
    fn replay_never_opens_a_socket() {
        let mut opts = fixture_opts(&temp("no-socket"));
        opts.subgraph = Some("http://127.0.0.1:9/nothing-listens-here".into());
        let outcome = run_with_key(&opts, Some("secret".into())).unwrap();
        assert_eq!(outcome.header.decided, 61);
        let output = read_output(&outcome);
        assert_eq!(
            record_for(&output, MRE7)["provenance"]["subgraph"]["source"],
            "replay"
        );
    }

    /// 05 R11: the fixture output matches the checked-in expectation,
    /// modulo the build identity.
    #[test]
    fn fixture_decisions_match_expected_json() {
        let outcome = run_with_key(&fixture_opts(&temp("expected")), None).unwrap();
        let mut actual = read_output(&outcome);
        let expected_path = fixture_dir().join("expected-decisions.json");
        let mut expected: Value =
            serde_json::from_str(&fs::read_to_string(&expected_path).unwrap()).unwrap();
        for output in [&mut actual, &mut expected] {
            for record in output["decisions"].as_array_mut().unwrap() {
                record["agent"]["git_commit"] = json!("<build>");
                record["record_sha256"] = json!("<build>");
            }
        }
        assert_eq!(
            actual, expected,
            "regenerate with the demo command and review the diff"
        );
    }

    /// 05 R11, 04 R16: the query files on disk hash to the values in the
    /// fixture records.
    #[test]
    fn queries_on_disk_match_the_hashes_in_the_fixture_records() {
        let expected: Value = serde_json::from_str(
            &fs::read_to_string(fixture_dir().join("expected-decisions.json")).unwrap(),
        )
        .unwrap();
        let queries = expected["decisions"][0]["provenance"]["queries"]
            .as_array()
            .unwrap();
        assert_eq!(queries.len(), 4);
        for query in queries {
            let name = query["name"].as_str().unwrap();
            let file =
                fs::read(repo_root().join(format!("subgraph/queries/{name}.graphql"))).unwrap();
            assert_eq!(
                query["query_sha256"],
                sha256_hex(&file),
                "{name}.graphql changed"
            );
        }
    }

    /// 05 R12: the endpoint is redacted and the key never reaches a file.
    #[test]
    fn consume_redacts_the_endpoint_and_key() {
        let key = "graph-key-0123456789abcdef0123456789abcdef";
        let mut opts = fixture_opts(&temp("redact"));
        opts.subgraph = Some(format!(
            "https://gateway.thegraph.com/api/{key}/subgraphs/id/ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890abcd"
        ));
        let outcome = run_with_key(&opts, Some(key.into())).unwrap();
        let text = fs::read_to_string(&outcome.decisions_path).unwrap();
        assert!(!text.contains(key), "key in decisions.json");
        let output: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            record_for(&output, MRE7)["provenance"]["subgraph"]["endpoint"],
            "https://gateway.thegraph.com/api/<redacted>/subgraphs/id/<redacted>"
        );
        for entry in fs::read_dir(outcome.out_dir.join("responses")).unwrap() {
            let body = fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!body.contains(key));
        }
    }

    /// 05 R14: the demo beat on the fixture.
    #[test]
    fn demo_beat_svzchf_allow_mre7_review() {
        let outcome = run_with_key(&fixture_opts(&temp("demo-beat")), None).unwrap();
        let output = read_output(&outcome);
        let svzchf = record_for(&output, SVZCHF);
        assert_eq!(svzchf["decision"], "ALLOW");
        assert_eq!(svzchf["reason"], Value::Null);
        let root = svzchf["evidence"]["crossfoot"]["bundle_root"]
            .as_str()
            .unwrap();
        assert_eq!(
            svzchf["reason_text"],
            format!(
                "MODEL_MATCH: 5 of 5 fields exact, residual 0 at block 25853000; bundle {root}"
            )
        );
        let mre7 = record_for(&output, MRE7);
        assert_eq!(mre7["decision"], "REVIEW");
        assert_eq!(mre7["reason"], "ADMIN_GUARD_BYPASSED");
        assert_eq!(mre7["evidence"]["subgraph"]["timeline"]["round_count"], 56);
        assert_eq!(
            mre7["evidence"]["subgraph"]["timeline"]["over_bound_round_ids"],
            json!(["36"])
        );

        let bypassed = output["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["reason"] == "ADMIN_GUARD_BYPASSED")
            .count();
        assert!(
            bypassed >= 14,
            "{bypassed} feeds REVIEW for ADMIN_GUARD_BYPASSED"
        );
        assert_eq!(output["header"]["decided"], 61);
        assert_eq!(
            output["header"]["allow"].as_u64().unwrap()
                + output["header"]["review"].as_u64().unwrap(),
            61
        );
        assert_eq!(output["header"]["wrappers"].as_array().unwrap().len(), 6);
        assert_eq!(output["header"]["unindexed"], json!([]));
        // Feeds are sorted by address.
        let addresses: Vec<&str> = output["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["feed"]["address"].as_str().unwrap())
            .collect();
        let mut sorted = addresses.clone();
        sorted.sort();
        assert_eq!(addresses, sorted);
    }

    /// 05 R15: the failure exits are errors from `run`, which main maps to 1.
    #[test]
    fn exit_codes_for_unreachable_endpoint_and_missing_feeds() {
        // Unreachable endpoint (network mode, nothing listens on port 9).
        let mut opts = fixture_opts(&temp("unreachable"));
        opts.replay = None;
        opts.subgraph = Some("http://127.0.0.1:9/".into());
        let err = run_with_key(&opts, None).unwrap_err();
        assert!(err.contains("did not answer"), "{err}");
        assert!(!err.contains("nothing"), "{err}");

        // Missing feeds.json, even in replay mode, and before any query.
        let mut opts = fixture_opts(&temp("missing-feeds"));
        opts.feeds = PathBuf::from("/nonexistent/feeds.json");
        let err = run_with_key(&opts, None).unwrap_err();
        assert!(err.contains("feeds.json"), "{err}");

        // A response that does not parse.
        let s = Synthetic::new("bad-response");
        fs::write(s.dir.join("responses/WindowFindings.json"), "not json").unwrap();
        let err = run_with_key(&s.opts(), None).unwrap_err();
        assert!(err.contains("WindowFindings"), "{err}");

        // A GraphQL error body.
        let s = Synthetic::new("graphql-error");
        s.write(
            "WindowFindings.json",
            &json!({"errors": [{"message": "boom"}]}),
        );
        let err = run_with_key(&s.opts(), None).unwrap_err();
        assert!(err.contains("boom"), "{err}");

        // No endpoint at all in network mode.
        let mut opts = fixture_opts(&temp("no-endpoint"));
        opts.replay = None;
        opts.subgraph = None;
        std::env::remove_var(ENV_SUBGRAPH_URL);
        let err = run_with_key(&opts, None).unwrap_err();
        assert!(err.contains("--subgraph"), "{err}");
    }

    /// The printed rows carry product.key and the decision words only.
    #[test]
    fn printed_rows_use_product_and_key() {
        let outcome = run_with_key(&fixture_opts(&temp("rows")), None).unwrap();
        assert!(outcome
            .rows
            .iter()
            .any(|(label, d, r)| label == "mRE7.customFeed"
                && *d == Decision::Review
                && r.as_deref() == Some("ADMIN_GUARD_BYPASSED")));
        assert!(outcome
            .rows
            .iter()
            .any(|(label, d, _)| label == "svZCHF" && *d == Decision::Allow));
    }

    /// 05 R14 live: 61 decisions from the Studio endpoint, deployment equal
    /// to subgraph/DEPLOYMENT.md. Needs CROSSFOOT_SUBGRAPH_URL.
    #[test]
    #[ignore]
    fn c1_consume_against_the_studio_endpoint() {
        let endpoint = std::env::var(ENV_SUBGRAPH_URL).expect("CROSSFOOT_SUBGRAPH_URL");
        let mut opts = fixture_opts(&temp("live"));
        opts.replay = None;
        opts.subgraph = Some(endpoint);
        opts.now = None;
        opts.block = Some(FIXTURE_BLOCK);
        let outcome = run(&opts).unwrap();
        assert_eq!(outcome.header.decided, 61);
        let deployment_md = fs::read_to_string(repo_root().join("subgraph/DEPLOYMENT.md")).unwrap();
        assert!(
            deployment_md.contains(&outcome.deployment),
            "deployment {} not in DEPLOYMENT.md",
            outcome.deployment
        );
    }
}
