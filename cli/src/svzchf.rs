//! The svZCHF fetch plan.
//!
//! Reads the Frankencoin savings vault and the savings module it actually
//! uses, at one pinned block, and sweeps the module's event history. Every
//! read is an eth_call at an explicit block number; nothing is read at
//! "latest", so a rerun at the same block is the same read.
//!
//! The module address is the one the vault reports through savings() on chain
//! (0x27d9AD98), not the SavingsV2 address the Frankencoin docs page links.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::{
    encode_address, encode_no_args, encode_uint256, Decoded, Expect, Field, FieldKind,
};
use crate::bundle::BundleWriter;
use crate::rpc::{
    blockscout_logs_descriptor, call_descriptor, chain_id_descriptor, get_block_descriptor,
    get_code_descriptor, get_logs_descriptor, Client, Fetched, RpcErrorKind, BLOCKSCOUT_RESULT_CAP,
};
use crate::util::{
    block_hex, git_provenance, now_stamp, now_utc, parse_hex_u64, workspace_packages,
};

pub const VAULT: &str = "0xE5F130253fF137f9917C0107659A4c5262abf6b0";
pub const MODULE: &str = "0x27d9AD987BdE08a0d083ef7e0e4043C857A17B38";
pub const EXPECTED_CHAIN_ID: u64 = 1;
pub const ONE_ETHER: u128 = 1_000_000_000_000_000_000;

/// Starting eth_getLogs span. drpc's free plan refuses ranges over 10000
/// blocks (observed during development); the sweep halves
/// this on a RequestTooBroad answer rather than assuming any particular
/// limit stays fixed.
const INITIAL_LOG_CHUNK: u64 = 10_000;
const MIN_LOG_CHUNK: u64 = 500;
/// Consecutive successful chunks before the sweep widens again.
const REGROW_AFTER: usize = 20;

/// Event signatures seen on this module. Each was confirmed by matching the
/// full 32 byte keccak of the signature against an observed topic0, so the
/// signature string is exact. What the contract does with each event is a
/// separate question and is not asserted here.
const KNOWN_EVENTS: [&str; 5] = [
    "Saved(address,uint192)",
    "Withdrawn(address,uint192)",
    "InterestCollected(address,uint256,uint256)",
    "RateChanged(uint24)",
    "RateProposed(address,uint24,uint40)",
];

pub const RATE_CHANGED_TOPIC0: &str =
    "0xd76dfbd4c35cffe2a846b6488bc677c511aa4337e1551f3a360427ac7a78de7b";
pub const SAVED_TOPIC0: &str = "0xf195ce54b48d5147da31c1fc525c8828b8836088b505a329e5de2b35da6731e2";
pub const WITHDRAWN_TOPIC0: &str =
    "0x47cf194f5e559cca0413017d38814a7843cc6f3052bc43c8085938774ae58151";
pub const INTEREST_COLLECTED_TOPIC0: &str =
    "0x9bbd517758fbae61197f1c1c04c8614064e89512dbaf4350dcdf76fcaa5e2161";
/// Emitted by the vault, not the module. Bookkeeping only, used as a
/// cross-check on the module's InterestCollected series.
pub const INTEREST_CLAIMED_TOPIC0: &str =
    "0x3c3606ed6d5dfe840e9ac3e4e9ff72ed55b257bdee70eb24a2ade297d439976e";

/// The module's deployment block. The constructor emits the first
/// RateChanged, so a fetch from here is self anchoring.
pub const MODULE_DEPLOYMENT_BLOCK: u64 = 22_536_327;

/// An address left padded to a 32 byte log topic.
pub fn address_topic(address: &str) -> String {
    format!("0x{:0>64}", address.trim_start_matches("0x").to_lowercase())
}

/// The account tuple returned by the module's savings(address).
pub const SAVINGS_ACCOUNT: [Field; 4] = [
    Field {
        name: "saved",
        kind: FieldKind::Uint,
    },
    Field {
        name: "ticks",
        kind: FieldKind::Uint,
    },
    Field {
        name: "referrer",
        kind: FieldKind::Address,
    },
    Field {
        name: "referralFeePPM",
        kind: FieldKind::Uint,
    },
];

/// Cross-check reference for the administered rate path, as (block, ppm).
/// It was recorded independently at development time, not read from a
/// primary source here. It exists so a divergence between what this tool fetches and what
/// that analysis reported becomes a recorded finding instead of passing
/// unnoticed. The fetched series is never adjusted to match it.
const REFERENCE_RATE_SERIES: [(u64, u64); 4] = [
    (22_536_327, 30_000),
    (23_983_764, 40_000),
    (24_426_856, 37_500),
    (24_750_879, 35_000),
];
const REFERENCE_RATE_SERIES_SOURCE: &str =
    "reference series recorded independently at development time, not verified against a primary source here";

/// Smallest Blockscout block window before the sweep gives up narrowing.
const MIN_BLOCKSCOUT_WINDOW: u64 = 10_000;

fn topic0_signature(topic0: &str) -> Option<&'static str> {
    KNOWN_EVENTS.into_iter().find(|signature| {
        format!(
            "0x{}",
            crate::abi::hex_encode(&crate::abi::keccak256(signature.as_bytes()))
        ) == topic0.to_lowercase()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    /// Blockscout's keyless API. One request covers the whole rate history.
    Blockscout,
    /// eth_getLogs against the configured RPC endpoints, chunked.
    Rpc,
    /// Fetch no log history at all.
    None,
}

pub struct FetchArgs {
    pub block: u64,
    pub baseline_block: Option<u64>,
    pub log_source: LogSource,
    pub full_log_history: bool,
    pub max_log_chunks: Option<usize>,
    /// Starting eth_getLogs span. Narrowing can still reduce it, and every
    /// narrowing is recorded in the manifest, so a run can be replayed
    /// exactly by passing the span the manifest reports.
    pub log_chunk: u64,
}

/// What `crossfoot fetch svzchf` reports after writing its own bundle.
pub struct Outcome {
    pub bundle_dir: std::path::PathBuf,
    pub network_calls: usize,
    pub cache_hits: usize,
    pub entry_count: usize,
    pub findings: Vec<crate::bundle::Finding>,
    pub flow_events: Vec<FlowEvent>,
    /// The manifest summary of the fetch: reads, rate history, flow series.
    pub summary: Value,
}

/// One pinned fetch recorded into a caller's bundle. The run command records
/// both of its fetches into one bundle this way, so the run is self
/// contained (spec 01 R7, spec 03 R1).
pub struct Fetch {
    /// The manifest summary of the fetch: reads, rate history, flow series.
    pub summary: Value,
    pub chain_id: u64,
    pub block_timestamp: Option<u64>,
    pub flow_events: Vec<FlowEvent>,
    /// The findings this fetch added to the bundle.
    pub findings: Vec<crate::bundle::Finding>,
}

/// One eth_call: fetch, decode, record. A revert or empty return data is a
/// finding recorded in the bundle, not a failure of the run.
fn read_call(
    client: &mut Client,
    bundle: &mut BundleWriter,
    label: &str,
    to: &str,
    calldata: &str,
    block_hex_value: &str,
    expect: Expect,
) -> Result<Option<Decoded>, String> {
    let descriptor = call_descriptor(label, to, calldata, block_hex_value);
    let fetched = client.fetch(descriptor).map_err(|err| err.message)?;

    match fetched.result_str() {
        Ok(data) => {
            let decoded = crate::abi::decode_return(&data, expect);
            let finding = if matches!(decoded, Decoded::Empty) {
                bundle.add_finding(
                    "empty_return_data",
                    label,
                    "the call returned zero bytes, so the function is absent or returned nothing",
                );
                Some("empty_return_data".to_string())
            } else {
                None
            };
            bundle
                .record(&fetched, Some(decoded.clone()), finding)
                .map_err(|e| e.to_string())?;
            Ok(Some(decoded))
        }
        Err(description) => {
            bundle.add_finding("call_reverted", label, description.clone());
            bundle
                .record(&fetched, None, Some("call_reverted".to_string()))
                .map_err(|e| e.to_string())?;
            Ok(None)
        }
    }
}

fn decoded_decimal(decoded: &Option<Decoded>) -> Option<String> {
    match decoded {
        Some(Decoded::Word { decimal, .. }) => Some(decimal.clone()),
        _ => None,
    }
}

fn decoded_address(decoded: &Option<Decoded>) -> Option<String> {
    match decoded {
        Some(Decoded::Word { address, .. }) => address.clone(),
        _ => None,
    }
}

fn has_code(fetched: &Fetched) -> Result<bool, String> {
    let code = fetched.result_str()?;
    Ok(code.len() > 2)
}

/// First block at which the address has code, by binary search over
/// eth_getCode. All probes are cached, so the search costs about 25 requests
/// once and nothing on later runs.
fn find_deployment_block(
    client: &mut Client,
    bundle: &mut BundleWriter,
    address: &str,
    upper: u64,
) -> Result<Option<u64>, String> {
    let probe =
        |client: &mut Client, bundle: &mut BundleWriter, block: u64| -> Result<bool, String> {
            let hex = block_hex(block);
            let label = format!("deployment probe eth_getCode @ {block}");
            let fetched = client
                .fetch(get_code_descriptor(&label, address, &hex))
                .map_err(|err| err.message)?;
            let present = has_code(&fetched)?;
            bundle
                .record(
                    &fetched,
                    Some(Decoded::Other {
                        hex: format!("code_present={present}"),
                        byte_len: 0,
                    }),
                    None,
                )
                .map_err(|e| e.to_string())?;
            Ok(present)
        };

    if !probe(client, bundle, upper)? {
        bundle.add_finding(
            "no_code_at_pinned_block",
            address,
            "the address has no code at the pinned block, so no event history was swept",
        );
        return Ok(None);
    }
    if probe(client, bundle, 0)? {
        bundle.add_finding(
            "code_at_genesis",
            address,
            "eth_getCode reports code at block 0, which the deployment search cannot explain",
        );
        return Ok(None);
    }

    let mut low = 0u64; // known to have no code
    let mut high = upper; // known to have code
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if probe(client, bundle, mid)? {
            high = mid;
        } else {
            low = mid;
        }
    }
    Ok(Some(high))
}

/// Sweeps the module's logs over [from, to] in chunks, narrowing the chunk
/// when an endpoint says the range is too broad.
#[allow(clippy::too_many_arguments)]
fn sweep_logs(
    client: &mut Client,
    bundle: &mut BundleWriter,
    address: &str,
    from: u64,
    to: u64,
    max_chunks: Option<usize>,
    initial_chunk: u64,
) -> Result<Value, String> {
    let mut chunk = initial_chunk.clamp(MIN_LOG_CHUNK, INITIAL_LOG_CHUNK);
    let mut cursor = from;
    let mut chunks_done = 0usize;
    let mut total_logs = 0usize;
    let mut topic0_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut narrowed_at: Vec<Value> = Vec::new();
    let mut truncated = false;
    let mut sweep_error: Option<String> = None;
    // Narrowing is not sticky: a single bad chunk must not halve throughput
    // for the rest of a two million block sweep.
    let mut consecutive_ok = 0usize;
    let mut regrow_events: Vec<Value> = Vec::new();

    while cursor <= to {
        if let Some(max) = max_chunks {
            if chunks_done >= max {
                truncated = true;
                bundle.add_finding(
                    "log_sweep_truncated",
                    address,
                    format!(
                        "stopped after {max} chunks at block {cursor}; the range {cursor}..{to} was not swept"
                    ),
                );
                break;
            }
        }
        let chunk_end = (cursor + chunk - 1).min(to);
        let from_hex = block_hex(cursor);
        let to_hex = block_hex(chunk_end);
        let label = format!("module logs {cursor}..{chunk_end}");
        let descriptor = get_logs_descriptor(&label, address, &from_hex, &to_hex);

        match client.fetch(descriptor) {
            Ok(fetched) => {
                let logs = match fetched.result() {
                    Ok(logs) => logs,
                    Err(err) => {
                        truncated = true;
                        sweep_error = Some(err.clone());
                        bundle.add_finding(
                            "log_sweep_incomplete",
                            address,
                            format!("eth_getLogs {cursor}..{chunk_end} returned an error: {err}"),
                        );
                        break;
                    }
                };
                let entries = logs.as_array().cloned().unwrap_or_default();
                total_logs += entries.len();
                for log in &entries {
                    if let Some(topic0) = log
                        .get("topics")
                        .and_then(Value::as_array)
                        .and_then(|topics| topics.first())
                        .and_then(Value::as_str)
                    {
                        *topic0_counts.entry(topic0.to_string()).or_insert(0) += 1;
                    }
                }
                bundle
                    .record(
                        &fetched,
                        Some(Decoded::Other {
                            hex: format!("logs={}", entries.len()),
                            byte_len: entries.len(),
                        }),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                chunks_done += 1;
                cursor = chunk_end + 1;
                consecutive_ok += 1;
                if consecutive_ok >= REGROW_AFTER && chunk < initial_chunk {
                    let wider = (chunk * 2).min(initial_chunk);
                    regrow_events.push(json!({
                        "at_block": cursor,
                        "from_chunk": chunk,
                        "to_chunk": wider,
                    }));
                    chunk = wider;
                    consecutive_ok = 0;
                }
            }
            Err(err) => match err.kind {
                // Any chunk failure is treated as narrowable while there is
                // room to narrow. The endpoint's own explanation is not
                // trusted: eth.drpc.org returned "ranges over 10000 blocks
                // are not supported" for a span it had just served 175 times,
                // so the message is recorded and the request is shrunk
                // regardless of what it claims.
                RpcErrorKind::RequestTooBroad | RpcErrorKind::Failed if chunk > MIN_LOG_CHUNK => {
                    let narrower = (chunk / 2).max(MIN_LOG_CHUNK);
                    narrowed_at.push(json!({
                        "at_block": cursor,
                        "from_chunk": chunk,
                        "to_chunk": narrower,
                        "endpoint_said": err.message,
                    }));
                    chunk = narrower;
                    consecutive_ok = 0;
                }
                // An endpoint that stops answering is an input gap, not a
                // crash. The bundle is still written, with the uncovered
                // range named, so the gap is visible instead of silent.
                _ => {
                    truncated = true;
                    sweep_error = Some(err.message.clone());
                    bundle.add_finding(
                        "log_sweep_incomplete",
                        address,
                        format!(
                            "the sweep stopped at block {cursor}; the range {cursor}..{to} was not swept. Endpoint reported: {}",
                            err.message
                        ),
                    );
                    break;
                }
            },
        }
    }

    Ok(json!({
        "address": address,
        "from_block": from,
        "to_block": to,
        "initial_chunk_size": initial_chunk,
        "final_chunk_size": chunk,
        "chunks_fetched": chunks_done,
        "chunk_narrowing_events": narrowed_at,
        "chunk_regrow_events": regrow_events,
        "truncated": truncated,
        "covered_to_block": if truncated { cursor.saturating_sub(1) } else { to },
        "sweep_error": sweep_error,
        "total_logs": total_logs,
        "topic0_histogram": topic0_counts
            .iter()
            .map(|(topic0, count)| {
                json!({
                    "topic0": topic0,
                    "count": count,
                    "signature": topic0_signature(topic0),
                })
            })
            .collect::<Vec<Value>>(),
    }))
}

/// Decodes the RateChanged series from Blockscout log rows. Each row carries
/// blockNumber, timeStamp and a single data word holding the new rate in ppm.
fn decode_rate_series(logs: &[Value]) -> Result<Vec<Value>, String> {
    let mut series = Vec::new();
    for log in logs {
        let block = log
            .get("blockNumber")
            .and_then(Value::as_str)
            .and_then(parse_hex_u64)
            .ok_or_else(|| format!("log row has no readable blockNumber: {log}"))?;
        let timestamp = log
            .get("timeStamp")
            .and_then(Value::as_str)
            .and_then(parse_hex_u64);
        let data = log
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("log row has no data field: {log}"))?;
        let rate = parse_hex_u64(data)
            .ok_or_else(|| format!("RateChanged data is not a readable word: {data}"))?;
        series.push(json!({
            "block": block,
            "timestamp_unix": timestamp,
            "timestamp_utc": timestamp.and_then(|ts| crate::util::unix_to_utc(ts as i64)),
            "rate_ppm": rate,
            "transaction_hash": log.get("transactionHash"),
        }));
    }
    Ok(series)
}

/// Compares the fetched rate series against the declared reference. A
/// difference is a finding, never a correction.
fn cross_check_rate_series(bundle: &mut BundleWriter, series: &[Value], to_block: u64) -> Value {
    let observed: Vec<(u64, u64)> = series
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("block")?.as_u64()?,
                entry.get("rate_ppm")?.as_u64()?,
            ))
        })
        .collect();
    // Only compare the part of the reference that falls inside the range this
    // run actually covered.
    let expected: Vec<(u64, u64)> = REFERENCE_RATE_SERIES
        .into_iter()
        .filter(|(block, _)| *block <= to_block)
        .collect();
    let matches = observed == expected;
    if !matches {
        bundle.add_finding(
            "rate_series_mismatch",
            MODULE,
            format!(
                "fetched rate series {observed:?} differs from the reference series {expected:?} ({REFERENCE_RATE_SERIES_SOURCE}); the fetched series is reported unchanged"
            ),
        );
    }
    json!({
        "reference_source": REFERENCE_RATE_SERIES_SOURCE,
        "reference_within_range": expected
            .iter()
            .map(|(b, r)| json!({"block": b, "rate_ppm": r}))
            .collect::<Vec<Value>>(),
        "matches_reference": matches,
    })
}

/// The rate change history, in one Blockscout request. The filter keeps the
/// result far below the result cap, which is checked rather than assumed.
fn fetch_rate_history(
    client: &mut Client,
    bundle: &mut BundleWriter,
    address: &str,
    to_block: u64,
) -> Result<Value, String> {
    let descriptor = blockscout_logs_descriptor(
        "rate change history, blockscout",
        address,
        Some(RATE_CHANGED_TOPIC0),
        None,
        0,
        to_block,
    );
    let fetched = client.fetch(descriptor).map_err(|err| err.message)?;
    let logs = fetched
        .result()
        .map_err(|err| format!("Blockscout rate history failed: {err}"))?;
    let rows = logs.as_array().cloned().unwrap_or_default();

    let capped = rows.len() >= BLOCKSCOUT_RESULT_CAP;
    if capped {
        bundle.add_finding(
            "blockscout_result_cap",
            address,
            format!(
                "the rate history request returned {} rows, at or above the {BLOCKSCOUT_RESULT_CAP} row cap, so it is truncated and the series below is incomplete",
                rows.len()
            ),
        );
    }

    bundle
        .record(
            &fetched,
            Some(Decoded::Other {
                hex: format!("rate_changed_logs={}", rows.len()),
                byte_len: rows.len(),
            }),
            capped.then(|| "blockscout_result_cap".to_string()),
        )
        .map_err(|e| e.to_string())?;

    let series = decode_rate_series(&rows)?;

    // The series is only self anchoring because the constructor emits the
    // first RateChanged at deployment, which fixes ticks = 0 at that
    // timestamp. If the first log is not at the deployment block, the origin
    // is wrong and every tick value derived from it is wrong.
    match series.first().and_then(|entry| entry.get("block")).and_then(Value::as_u64) {
        Some(first) if first == MODULE_DEPLOYMENT_BLOCK => {}
        Some(first) => bundle.add_finding(
            "rate_series_not_anchored",
            address,
            format!(
                "the first RateChanged is at block {first}, not the deployment block {MODULE_DEPLOYMENT_BLOCK}, so the tick clock origin is not established"
            ),
        ),
        None => bundle.add_finding(
            "rate_series_empty",
            address,
            "no RateChanged logs were returned, so the tick clock has no origin",
        ),
    }

    let cross_check = cross_check_rate_series(bundle, &series, to_block);

    Ok(json!({
        "source": "blockscout",
        "topic0": RATE_CHANGED_TOPIC0,
        "signature": "RateChanged(uint24)",
        "from_block": 0,
        "to_block": to_block,
        "result_cap": BLOCKSCOUT_RESULT_CAP,
        "truncated_by_cap": capped,
        "series": series,
        "cross_check": cross_check,
    }))
}

/// The complete unfiltered event history, windowed. Blockscout caps a
/// response at 1000 rows and does not say so, and its page parameter is
/// ignored, so the only safe reading of a response at the cap is that the
/// window was too wide. The window halves until every response is under it.
fn sweep_blockscout_all(
    client: &mut Client,
    bundle: &mut BundleWriter,
    address: &str,
    from: u64,
    to: u64,
) -> Result<Value, String> {
    let mut window = to.saturating_sub(from) + 1;
    let mut cursor = from;
    let mut requests = 0usize;
    let mut total_logs = 0usize;
    let mut topic0_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut narrowed_at: Vec<Value> = Vec::new();
    let mut truncated = false;
    let mut sweep_error: Option<String> = None;

    while cursor <= to {
        let window_end = (cursor + window - 1).min(to);
        let label = format!("all module logs, blockscout {cursor}..{window_end}");
        let descriptor =
            blockscout_logs_descriptor(&label, address, None, None, cursor, window_end);
        match client.fetch(descriptor) {
            Ok(fetched) => {
                let rows = fetched
                    .result()
                    .map_err(|err| format!("Blockscout {cursor}..{window_end}: {err}"))?
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if rows.len() >= BLOCKSCOUT_RESULT_CAP && window > MIN_BLOCKSCOUT_WINDOW {
                    let narrower = (window / 2).max(MIN_BLOCKSCOUT_WINDOW);
                    narrowed_at.push(json!({
                        "at_block": cursor,
                        "from_window": window,
                        "to_window": narrower,
                        "reason": format!("{} rows returned, at or above the {BLOCKSCOUT_RESULT_CAP} row cap", rows.len()),
                    }));
                    window = narrower;
                    continue;
                }
                if rows.len() >= BLOCKSCOUT_RESULT_CAP {
                    truncated = true;
                    bundle.add_finding(
                        "blockscout_result_cap",
                        address,
                        format!(
                            "window {cursor}..{window_end} is already at the minimum and still returned {} rows, so this window is truncated",
                            rows.len()
                        ),
                    );
                }
                total_logs += rows.len();
                for log in &rows {
                    if let Some(topic0) = log
                        .get("topics")
                        .and_then(Value::as_array)
                        .and_then(|topics| topics.first())
                        .and_then(Value::as_str)
                    {
                        *topic0_counts.entry(topic0.to_string()).or_insert(0) += 1;
                    }
                }
                bundle
                    .record(
                        &fetched,
                        Some(Decoded::Other {
                            hex: format!("logs={}", rows.len()),
                            byte_len: rows.len(),
                        }),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                requests += 1;
                cursor = window_end + 1;
            }
            Err(err) => {
                truncated = true;
                sweep_error = Some(err.message.clone());
                bundle.add_finding(
                    "log_sweep_incomplete",
                    address,
                    format!(
                        "the Blockscout sweep stopped at block {cursor}; the range {cursor}..{to} was not swept. Endpoint reported: {}",
                        err.message
                    ),
                );
                break;
            }
        }
    }

    Ok(json!({
        "source": "blockscout",
        "from_block": from,
        "to_block": to,
        "covered_to_block": if truncated { cursor.saturating_sub(1) } else { to },
        "final_window": window,
        "requests": requests,
        "window_narrowing_events": narrowed_at,
        "truncated": truncated,
        "sweep_error": sweep_error,
        "total_logs": total_logs,
        "topic0_histogram": topic0_counts
            .iter()
            .map(|(topic0, count)| {
                json!({
                    "topic0": topic0,
                    "count": count,
                    "signature": topic0_signature(topic0),
                })
            })
            .collect::<Vec<Value>>(),
    }))
}

/// The fetch command: one pinned fetch in a bundle of its own.
pub fn run(client: &mut Client, args: &FetchArgs, verify_root: &Path) -> Result<Outcome, String> {
    let started = now_utc();
    let bundle_name = format!("svzchf-{}-{}", args.block, now_stamp());
    let mut bundle = BundleWriter::create(
        &verify_root.join("bundles"),
        &bundle_name,
        EXPECTED_CHAIN_ID,
    )
    .map_err(|err| format!("could not create the bundle directory: {err}"))?;

    let fetched = fetch(client, &mut bundle, args)?;
    let finished = now_utc();

    bundle
        .write_manifest("svzchf", fetched.summary.clone())
        .map_err(|err| format!("could not write manifest.json: {err}"))?;

    let meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "svzchf",
        "repo_git": git_provenance(verify_root),
        "workspace_packages": workspace_packages(),
        "chain_id": fetched.chain_id,
        "chain_id_source": "eth_chainId",
        "block": args.block,
        "block_hex": block_hex(args.block),
        "block_timestamp_unix": fetched.block_timestamp,
        "baseline_block": args.baseline_block,
        "endpoints_configured": client.endpoints(),
        "log_endpoints_configured": client.log_endpoints(),
        "cache_root": client.cache().root().display().to_string(),
        "fetch_started_utc": started,
        "fetch_finished_utc": finished,
        "network_calls_this_run": client.network_calls,
        "cache_hits_this_run": client.cache_hits,
        "rpc_observations": client.observations,
        "endpoint_fingerprints": client.endpoint_fingerprints(),
    });
    bundle
        .write_meta(meta)
        .map_err(|err| format!("could not write meta.json: {err}"))?;

    Ok(Outcome {
        bundle_dir: bundle.dir().to_path_buf(),
        network_calls: client.network_calls,
        cache_hits: client.cache_hits,
        entry_count: bundle.entries().len(),
        findings: bundle.findings().to_vec(),
        flow_events: fetched.flow_events,
        summary: fetched.summary,
    })
}

/// The fetch plan, recorded into the caller's bundle. Every read is an
/// eth_call at an explicit block number; the caller decides which bundle the
/// evidence lands in.
pub fn fetch(
    client: &mut Client,
    bundle: &mut BundleWriter,
    args: &FetchArgs,
) -> Result<Fetch, String> {
    let findings_before = bundle.findings().len();
    let block_hex_value = block_hex(args.block);

    // 1. Chain identity. A bundle that does not know which chain it read is
    //    worthless, so a mismatch stops the run.
    let chain_fetched = client
        .fetch(chain_id_descriptor())
        .map_err(|err| err.message)?;
    let chain_id_hex = chain_fetched.result_str()?;
    let chain_id = parse_hex_u64(&chain_id_hex)
        .ok_or_else(|| format!("eth_chainId returned an unparsable value: {chain_id_hex}"))?;
    bundle
        .record(&chain_fetched, None, None)
        .map_err(|e| e.to_string())?;
    if chain_id != EXPECTED_CHAIN_ID {
        return Err(format!(
            "endpoint reports chain id {chain_id}, expected {EXPECTED_CHAIN_ID} (Ethereum mainnet)"
        ));
    }

    // 2. The pinned block header, for its timestamp.
    let block_fetched = client
        .fetch(get_block_descriptor(
            &format!("block header @ {}", args.block),
            &block_hex_value,
        ))
        .map_err(|err| err.message)?;
    let block_value = block_fetched.result()?;
    let block_timestamp = block_value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64);
    let block_hash = block_value
        .get("hash")
        .and_then(Value::as_str)
        .map(str::to_string);
    bundle
        .record(&block_fetched, None, None)
        .map_err(|e| e.to_string())?;
    if block_timestamp.is_none() {
        bundle.add_finding(
            "missing_block_timestamp",
            "block header",
            "eth_getBlockByNumber returned no timestamp for the pinned block",
        );
    }

    // 3. Vault reads at the pinned block.
    let asset = read_call(
        client,
        bundle,
        "vault.asset()",
        VAULT,
        &encode_no_args("asset()"),
        &block_hex_value,
        Expect::Address,
    )?;
    let savings = read_call(
        client,
        bundle,
        "vault.savings()",
        VAULT,
        &encode_no_args("savings()"),
        &block_hex_value,
        Expect::Address,
    )?;
    let total_supply = read_call(
        client,
        bundle,
        "vault.totalSupply()",
        VAULT,
        &encode_no_args("totalSupply()"),
        &block_hex_value,
        Expect::Uint,
    )?;
    let total_assets = read_call(
        client,
        bundle,
        "vault.totalAssets()",
        VAULT,
        &encode_no_args("totalAssets()"),
        &block_hex_value,
        Expect::Uint,
    )?;
    let convert_to_assets = read_call(
        client,
        bundle,
        "vault.convertToAssets(1e18)",
        VAULT,
        &encode_uint256("convertToAssets(uint256)", ONE_ETHER),
        &block_hex_value,
        Expect::Uint,
    )?;

    // The vault must point at the module this run reads, otherwise the two
    // halves of the bundle describe different systems.
    if let Some(reported) = decoded_address(&savings) {
        if reported.to_lowercase() != MODULE.to_lowercase() {
            bundle.add_finding(
                "module_address_mismatch",
                "vault.savings()",
                format!("vault.savings() reports {reported}, this run read module {MODULE}"),
            );
        }
    }

    // 4. Module reads at the pinned block.
    let current_rate = read_call(
        client,
        bundle,
        "module.currentRatePPM()",
        MODULE,
        &encode_no_args("currentRatePPM()"),
        &block_hex_value,
        Expect::Uint,
    )?;
    let current_ticks = read_call(
        client,
        bundle,
        "module.currentTicks()",
        MODULE,
        &encode_no_args("currentTicks()"),
        &block_hex_value,
        Expect::Uint,
    )?;
    let interest_delay = read_call(
        client,
        bundle,
        "module.INTEREST_DELAY()",
        MODULE,
        &encode_no_args("INTEREST_DELAY()"),
        &block_hex_value,
        Expect::Uint,
    )?;

    // The vault's own price, and the vault's account inside the module. These
    // are the comparison targets the recompute step comes back for, so they
    // are captured in the same pinned bundle rather than in a second pass.
    let price = read_call(
        client,
        bundle,
        "vault.price()",
        VAULT,
        &encode_no_args("price()"),
        &block_hex_value,
        Expect::Uint,
    )?;
    let account = read_call(
        client,
        bundle,
        "module.savings(vault)",
        MODULE,
        &encode_address("savings(address)", VAULT)?,
        &block_hex_value,
        Expect::Fields(&SAVINGS_ACCOUNT),
    )?;

    // ticks() takes a timestamp, so it can only be built once the pinned
    // block header has been read.
    let ticks_at_block = match block_timestamp {
        Some(timestamp) => read_call(
            client,
            bundle,
            &format!("module.ticks({timestamp})"),
            MODULE,
            &encode_uint256("ticks(uint256)", timestamp as u128),
            &block_hex_value,
            Expect::Uint,
        )?,
        None => {
            bundle.add_finding(
                "ticks_not_read",
                "module.ticks(uint256)",
                "the pinned block header carried no timestamp, so ticks(timestamp) could not be built",
            );
            None
        }
    };

    // price() and convertToAssets(1e18) are expected to agree. If they ever
    // stop agreeing, that is a property of the contract worth recording.
    if let (Some(a), Some(b)) = (decoded_decimal(&price), decoded_decimal(&convert_to_assets)) {
        if a != b {
            bundle.add_finding(
                "price_convert_mismatch",
                VAULT,
                format!("price() returned {a} while convertToAssets(1e18) returned {b}"),
            );
        }
    }

    // 5. Baseline block, when given: the same exchange rate read plus its
    //    header, which is what a two block growth check needs.
    let mut baseline_summary = Value::Null;
    if let Some(baseline) = args.baseline_block {
        if baseline >= args.block {
            return Err(format!(
                "--baseline-block {baseline} must be below --block {}",
                args.block
            ));
        }
        let baseline_hex = block_hex(baseline);
        let baseline_header = client
            .fetch(get_block_descriptor(
                &format!("baseline block header @ {baseline}"),
                &baseline_hex,
            ))
            .map_err(|err| err.message)?;
        let baseline_ts = baseline_header.result().ok().and_then(|value| {
            value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_hex_u64)
        });
        bundle
            .record(&baseline_header, None, None)
            .map_err(|e| e.to_string())?;
        let baseline_rate = read_call(
            client,
            bundle,
            "baseline vault.convertToAssets(1e18)",
            VAULT,
            &encode_uint256("convertToAssets(uint256)", ONE_ETHER),
            &baseline_hex,
            Expect::Uint,
        )?;
        baseline_summary = json!({
            "block": baseline,
            "block_hex": baseline_hex,
            "timestamp_unix": baseline_ts,
            "convert_to_assets_1e18": decoded_decimal(&baseline_rate),
        });
    }

    // 6. Event history. Without a baseline the sweep starts at the module's
    //    own deployment block, found by binary search over eth_getCode.
    let mut deployment_block: Option<u64> = None;
    let mut log_summary = Value::Null;
    let mut rate_history = Value::Null;
    let mut flows = Value::Null;
    let mut interest_claimed = Value::Null;
    let mut flow_events: Vec<FlowEvent> = Vec::new();
    if args.log_source == LogSource::None {
        bundle.add_finding(
            "log_history_skipped",
            MODULE,
            "log history was not fetched, so this bundle carries no rate change history",
        );
    } else if args.log_source == LogSource::Blockscout {
        // One filtered request covers the whole rate path, which is the only
        // part of the event history the recompute needs.
        rate_history = fetch_rate_history(client, bundle, MODULE, args.block)?;
        let (events, flow_summary) = fetch_vault_flows(client, bundle, MODULE, VAULT, args.block)?;
        flow_events = events;
        flows = flow_summary;
        interest_claimed = fetch_interest_claimed(client, bundle, VAULT, args.block)?;
        // The vault emits one InterestClaimed per deposit or withdrawal it
        // routes, while the module emits InterestCollected only when the
        // accrued amount was nonzero, so the two counts are related but not
        // equal. A claimed count below the collected count would mean the
        // flow series is missing events.
        if let (Some(claimed), Some(collected)) = (
            interest_claimed.get("count").and_then(Value::as_u64),
            flows.get("interest_collected").and_then(Value::as_u64),
        ) {
            if claimed < collected {
                bundle.add_finding(
                    "flow_series_inconsistent",
                    VAULT,
                    format!(
                        "the vault emitted {claimed} InterestClaimed but the module emitted {collected} InterestCollected for it; the flow series looks incomplete"
                    ),
                );
            }
        }
        if args.full_log_history {
            let from = args.baseline_block.unwrap_or(0);
            log_summary = sweep_blockscout_all(client, bundle, MODULE, from, args.block)?;
        }
    } else {
        let from = match args.baseline_block {
            Some(baseline) => baseline,
            None => {
                let found = find_deployment_block(client, bundle, MODULE, args.block)?;
                deployment_block = found;
                match found {
                    Some(block) => block,
                    None => args.block,
                }
            }
        };
        if from <= args.block {
            log_summary = sweep_logs(
                client,
                bundle,
                MODULE,
                from,
                args.block,
                args.max_log_chunks,
                args.log_chunk,
            )?;
        }
    }

    let summary = json!({
        "vault": VAULT,
        "module": MODULE,
        "block": args.block,
        "block_hex": block_hex_value,
        "block_hash": block_hash,
        "block_timestamp_unix": block_timestamp,
        "block_timestamp_utc": block_timestamp.and_then(|ts| crate::util::unix_to_utc(ts as i64)),
        "reads": {
            "vault.asset()": decoded_address(&asset),
            "vault.savings()": decoded_address(&savings),
            "vault.totalSupply()": decoded_decimal(&total_supply),
            "vault.totalAssets()": decoded_decimal(&total_assets),
            "vault.convertToAssets(1e18)": decoded_decimal(&convert_to_assets),
            "module.currentRatePPM()": decoded_decimal(&current_rate),
            "module.currentTicks()": decoded_decimal(&current_ticks),
            "module.INTEREST_DELAY()": decoded_decimal(&interest_delay),
            "vault.price()": decoded_decimal(&price),
            "module.ticks(block timestamp)": decoded_decimal(&ticks_at_block),
            "module.savings(vault)": account.as_ref().and_then(|decoded| match decoded {
                Decoded::Fields { fields, .. } => Some(
                    fields
                        .iter()
                        .map(|field| {
                            let mut value = serde_json::Map::new();
                            value.insert("decimal".to_string(), json!(field.decimal));
                            if let Some(address) = &field.address {
                                value.insert("address".to_string(), json!(address));
                            }
                            (field.name.to_string(), Value::Object(value))
                        })
                        .collect::<serde_json::Map<String, Value>>(),
                ),
                _ => None,
            }),
        },
        "baseline": baseline_summary,
        "module_deployment_block": deployment_block,
        "rate_history": rate_history,
        "vault_flows": flows,
        "vault_interest_claimed": interest_claimed,
        "logs": log_summary,
    });

    Ok(Fetch {
        summary,
        chain_id,
        block_timestamp,
        flow_events,
        findings: bundle.findings()[findings_before..].to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Vault flow series
// ---------------------------------------------------------------------------

/// One recognition event on the vault's savings account, in chain order.
#[derive(Debug, Clone, Serialize)]
pub struct FlowEvent {
    pub block: u64,
    pub log_index: u64,
    pub timestamp: u64,
    pub kind: FlowKind,
    /// Saved and Withdrawn: the amount. InterestCollected: the gross interest.
    pub amount: String,
    /// InterestCollected only: the referral fee.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referral_fee: Option<String>,
    pub transaction_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    Saved,
    Withdrawn,
    InterestCollected,
}

fn log_u64(log: &Value, field: &str) -> Option<u64> {
    log.get(field)
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
}

/// Reads a 32 byte word out of a log data blob, as a decimal string.
fn data_word(data: &str, index: usize) -> Option<String> {
    let body = data.strip_prefix("0x").unwrap_or(data);
    let start = index * 64;
    let slice = body.get(start..start + 64)?;
    let bytes = crate::abi::hex_decode(slice)?;
    let mut word = [0u8; 32];
    word.copy_from_slice(&bytes);
    Some(crate::abi::word_to_decimal(&word))
}

fn decode_flow_events(rows: &[Value]) -> Result<Vec<FlowEvent>, String> {
    let mut events = Vec::new();
    for log in rows {
        let topic0 = log
            .get("topics")
            .and_then(Value::as_array)
            .and_then(|t| t.first())
            .and_then(Value::as_str)
            .ok_or_else(|| format!("log row has no topic0: {log}"))?
            .to_lowercase();
        let kind = if topic0 == SAVED_TOPIC0 {
            FlowKind::Saved
        } else if topic0 == WITHDRAWN_TOPIC0 {
            FlowKind::Withdrawn
        } else if topic0 == INTEREST_COLLECTED_TOPIC0 {
            FlowKind::InterestCollected
        } else {
            // An unrecognised event on this account is not silently dropped:
            // it would mean the account has a state transition the model does
            // not know about.
            return Err(format!(
                "unrecognised event {topic0} on the vault account; the model does not know this state transition"
            ));
        };
        let data = log
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("log row has no data: {log}"))?;
        events.push(FlowEvent {
            block: log_u64(log, "blockNumber")
                .ok_or_else(|| format!("log row has no blockNumber: {log}"))?,
            log_index: log_u64(log, "logIndex").unwrap_or(0),
            timestamp: log_u64(log, "timeStamp")
                .ok_or_else(|| format!("log row has no timeStamp: {log}"))?,
            kind,
            amount: data_word(data, 0)
                .ok_or_else(|| format!("log data is too short for one word: {data}"))?,
            referral_fee: if kind == FlowKind::InterestCollected {
                Some(
                    data_word(data, 1).ok_or_else(|| {
                        format!("InterestCollected data lacks a second word: {data}")
                    })?,
                )
            } else {
                None
            },
            transaction_hash: log
                .get("transactionHash")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    // Chain order, which is the order the contract applied them in.
    events.sort_by_key(|event| (event.block, event.log_index));
    Ok(events)
}

/// Every event on the module touching the vault's account, in one request.
/// Filtering by topic1 alone returns Saved, Withdrawn and InterestCollected
/// together, which is what the replay needs and keeps the ordering exact.
pub fn fetch_vault_flows(
    client: &mut Client,
    bundle: &mut BundleWriter,
    module: &str,
    vault: &str,
    to_block: u64,
) -> Result<(Vec<FlowEvent>, Value), String> {
    let descriptor = blockscout_logs_descriptor(
        "vault flow series, blockscout",
        module,
        None,
        Some(&address_topic(vault)),
        0,
        to_block,
    );
    let fetched = client.fetch(descriptor).map_err(|err| err.message)?;
    let rows = fetched
        .result()
        .map_err(|err| format!("Blockscout vault flow series failed: {err}"))?
        .as_array()
        .cloned()
        .unwrap_or_default();

    let capped = rows.len() >= BLOCKSCOUT_RESULT_CAP;
    if capped {
        bundle.add_finding(
            "blockscout_result_cap",
            module,
            format!(
                "the vault flow series returned {} rows, at or above the {BLOCKSCOUT_RESULT_CAP} row cap, so it is truncated and the replay would be built on an incomplete event list",
                rows.len()
            ),
        );
    }
    bundle
        .record(
            &fetched,
            Some(Decoded::Other {
                hex: format!("flow_logs={}", rows.len()),
                byte_len: rows.len(),
            }),
            capped.then(|| "blockscout_result_cap".to_string()),
        )
        .map_err(|e| e.to_string())?;

    let events = decode_flow_events(&rows)?;
    let counts = |kind: FlowKind| events.iter().filter(|e| e.kind == kind).count();
    let summary = json!({
        "source": "blockscout",
        "module": module,
        "account": vault,
        "from_block": 0,
        "to_block": to_block,
        "result_cap": BLOCKSCOUT_RESULT_CAP,
        "truncated_by_cap": capped,
        "total": events.len(),
        "saved": counts(FlowKind::Saved),
        "withdrawn": counts(FlowKind::Withdrawn),
        "interest_collected": counts(FlowKind::InterestCollected),
        "first_block": events.first().map(|e| e.block),
        "last_block": events.last().map(|e| e.block),
    });
    Ok((events, summary))
}

/// The vault's own InterestClaimed series, used only to cross-check the
/// module's InterestCollected count.
pub fn fetch_interest_claimed(
    client: &mut Client,
    bundle: &mut BundleWriter,
    vault: &str,
    to_block: u64,
) -> Result<Value, String> {
    let descriptor = blockscout_logs_descriptor(
        "vault InterestClaimed, blockscout",
        vault,
        Some(INTEREST_CLAIMED_TOPIC0),
        None,
        0,
        to_block,
    );
    let fetched = client.fetch(descriptor).map_err(|err| err.message)?;
    let rows = fetched
        .result()
        .map_err(|err| format!("Blockscout InterestClaimed failed: {err}"))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    bundle
        .record(
            &fetched,
            Some(Decoded::Other {
                hex: format!("interest_claimed_logs={}", rows.len()),
                byte_len: rows.len(),
            }),
            None,
        )
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "source": "blockscout",
        "vault": vault,
        "to_block": to_block,
        "count": rows.len(),
    }))
}
