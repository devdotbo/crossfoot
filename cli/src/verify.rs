//! `crossfoot verify <bundle>`: does the stated result follow from the
//! stated responses under the stated code, without the network?
//!
//! In order: parse the manifest, meta and result; re-hash every raw body
//! against its manifest entry; recompute every cache key from its preimage;
//! recompute SHA256SUMS and the root hash; replay the run through a
//! `BundleSource` into a temporary directory; compare the replayed
//! result.json byte for byte; compare the producer's code identity with this
//! binary's. One exit code says which step failed (spec 03 R8 to R12).
//!
//! What this proves: that result.json is exactly what this code computes
//! from the raw responses in the bundle, that no response was altered after
//! the manifest was written, and that no read outside the bundle was needed.
//! What it does not prove: that the responses are what the chain holds. A
//! node that lied consistently produces a bundle that verifies.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::bundle::{root_hash_of, sha256sums_text, MANIFEST_FORMAT};
use crate::cache::sha256_hex;
use crate::source::BundleSource;

/// The one-sentence scope statement the report, the README and the demo
/// use, verbatim.
pub const SCOPE_SENTENCE: &str = "A match proves that the stated result follows from the stated responses under the stated code. It does not prove that the responses are what the chain holds; for that, re-read the pinned blocks from an endpoint you trust, or run `verify --refetch`.";

pub const VERIFIED: u8 = 0;
pub const OTHER: u8 = 1;
pub const HASH_MISMATCH: u8 = 2;
pub const REPLAY_MISMATCH: u8 = 3;
pub const BUNDLE_INCOMPLETE: u8 = 4;
pub const CODE_MISMATCH: u8 = 5;
pub const REFETCH_MISMATCH: u8 = 6;

/// How many JSON-RPC entries `--refetch` re-reads from the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    All,
    Count(usize),
}

impl std::str::FromStr for Sample {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        if text == "all" {
            return Ok(Sample::All);
        }
        match text.parse::<usize>() {
            Ok(n) if n > 0 => Ok(Sample::Count(n)),
            _ => Err(format!(
                "--refetch takes a positive count or \"all\", not {text:?}"
            )),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Exit 5 when the producer's code identity differs.
    pub require_same_code: bool,
    /// Re-read a sample of JSON-RPC entries from the network (spec 03 R13).
    /// Without it verify opens no socket.
    pub refetch: Option<Sample>,
    /// Endpoints for the refetch; the defaults when empty.
    pub endpoints: Vec<String>,
}

pub struct Report {
    pub exit_code: u8,
    pub status: &'static str,
    pub lines: Vec<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            exit_code: VERIFIED,
            status: "VERIFIED",
            lines: Vec::new(),
        }
    }

    fn line(&mut self, key: &str, value: impl std::fmt::Display) {
        self.lines.push(format!("{key:<16}{value}"));
    }

    fn fail(mut self, exit_code: u8, status: &'static str, detail: impl std::fmt::Display) -> Self {
        self.exit_code = exit_code;
        self.status = status;
        self.line("status", format!("{status}: {detail}"));
        self.line("scope", SCOPE_SENTENCE);
        self
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("{} is not JSON: {err}", path.display()))
}

/// The first JSON path at which two values differ, with both values. Object
/// keys are compared as a sorted union so a missing key is a difference at
/// that key, not a shifted comparison of everything after it.
pub fn first_difference(a: &Value, b: &Value) -> Option<(String, Value, Value)> {
    fn walk(a: &Value, b: &Value, path: &str) -> Option<(String, Value, Value)> {
        match (a, b) {
            (Value::Object(left), Value::Object(right)) => {
                let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
                keys.sort();
                keys.dedup();
                for key in keys {
                    let here = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    match (left.get(key), right.get(key)) {
                        (Some(x), Some(y)) => {
                            if let Some(found) = walk(x, y, &here) {
                                return Some(found);
                            }
                        }
                        (x, y) => {
                            return Some((
                                here,
                                x.cloned().unwrap_or(Value::Null),
                                y.cloned().unwrap_or(Value::Null),
                            ))
                        }
                    }
                }
                None
            }
            (Value::Array(left), Value::Array(right)) => {
                for index in 0..left.len().max(right.len()) {
                    let here = format!("{path}[{index}]");
                    match (left.get(index), right.get(index)) {
                        (Some(x), Some(y)) => {
                            if let Some(found) = walk(x, y, &here) {
                                return Some(found);
                            }
                        }
                        (x, y) => {
                            return Some((
                                here,
                                x.cloned().unwrap_or(Value::Null),
                                y.cloned().unwrap_or(Value::Null),
                            ))
                        }
                    }
                }
                None
            }
            _ => (a != b).then(|| (path.to_string(), a.clone(), b.clone())),
        }
    }
    walk(a, b, "")
}

/// Steps b to d: every manifest entry against its file, every preimage
/// against its key, no file outside the manifest under raw/, and the
/// checksum list and root hash against the files. Returns the first failure.
fn check_hashes(dir: &Path, manifest: &Value, report: &mut Report) -> Result<String, String> {
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or("the manifest has no entries")?;
    let mut listed: Vec<String> = Vec::new();
    for entry in entries {
        let file = entry
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("a manifest entry has no file: {entry}"))?;
        listed.push(file.to_string());
        let bytes = fs::read(dir.join(file)).map_err(|err| format!("{file}: {err}"))?;
        let expected_sha = entry.get("sha256").and_then(Value::as_str).unwrap_or("");
        if sha256_hex(&bytes) != expected_sha {
            return Err(format!("{file}: sha256 differs from the manifest"));
        }
        let expected_len = entry.get("byte_len").and_then(Value::as_u64).unwrap_or(0);
        if bytes.len() as u64 != expected_len {
            return Err(format!(
                "{file}: {} bytes, the manifest says {expected_len}",
                bytes.len()
            ));
        }
        let preimage = entry
            .get("preimage")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{file}: the manifest entry has no preimage"))?;
        let key = entry.get("cache_key").and_then(Value::as_str).unwrap_or("");
        if sha256_hex(preimage.as_bytes()) != key {
            return Err(format!(
                "{file}: the cache key does not hash from the preimage"
            ));
        }
    }
    // A raw file the manifest does not list is as much a change as a
    // missing one.
    let raw_dir = dir.join("raw");
    if raw_dir.is_dir() {
        for entry in fs::read_dir(&raw_dir).map_err(|err| format!("raw/: {err}"))? {
            let entry = entry.map_err(|err| format!("raw/: {err}"))?;
            let name = format!("raw/{}", entry.file_name().to_string_lossy());
            if !listed.contains(&name) {
                return Err(format!("{name}: present but not in the manifest"));
            }
        }
    }
    report.line("entries", format!("{} checked, hashes ok", entries.len()));

    let sums_path = dir.join("SHA256SUMS");
    let recorded = fs::read_to_string(&sums_path).map_err(|err| format!("SHA256SUMS: {err}"))?;
    let recomputed = sha256sums_text(dir)?;
    if recorded != recomputed {
        // Name the first line that differs, so the reader knows which file.
        let culprit = recorded
            .lines()
            .zip(recomputed.lines())
            .find(|(a, b)| a != b)
            .map(|(a, _)| a.get(66..).unwrap_or(a).to_string())
            .unwrap_or_else(|| "the file list".to_string());
        return Err(format!("SHA256SUMS differs from the files at {culprit}"));
    }
    let root = root_hash_of(&recorded);
    let stated = fs::read_to_string(dir.join("bundle.sha256"))
        .map_err(|err| format!("bundle.sha256: {err}"))?;
    if stated.trim() != root {
        return Err("bundle.sha256 is not the sha256 of SHA256SUMS".to_string());
    }
    Ok(root)
}

/// Step e: the replay, into a temporary root, through the bundle. Returns
/// the replayed result.json path.
fn replay(
    dir: &Path,
    target: &str,
    result: &Value,
    meta: Option<&Value>,
    replay_root: &Path,
) -> Result<PathBuf, (u8, String)> {
    let mut source = BundleSource::open(dir).map_err(|err| (OTHER, err))?;
    let window = result
        .get("window")
        .ok_or((OTHER, "result.json has no window".to_string()))?;
    let block = window
        .get("block")
        .and_then(Value::as_u64)
        .ok_or((OTHER, "result.json window has no block".to_string()))?;
    let baseline_block = window
        .get("baseline_block")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let window_name = meta
        .and_then(|m| m.get("window"))
        .and_then(|w| w.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    fs::create_dir_all(replay_root).map_err(|err| {
        (
            OTHER,
            format!("could not create {}: {err}", replay_root.display()),
        )
    })?;

    let outcome: Result<PathBuf, String> = match target {
        "svzchf" => crate::run_svzchf::run(
            &mut source,
            &crate::run_svzchf::RunArgs {
                baseline_block,
                block,
                window_name,
            },
            replay_root,
        )
        .map(|outcome| outcome.result_path),
        "mtbill" => crate::run_mtbill::run(
            &mut source,
            &crate::run_mtbill::RunArgs {
                baseline_block,
                block,
                window_name,
            },
            replay_root,
        )
        .map(|outcome| outcome.result_path),
        // A posted-feed family run (midas or any family config): the manifest
        // summary carries the mechanism. Spec 03 R7: the feed list comes from
        // the bundle's manifest, not from the working tree's config file.
        _ if read_json(&dir.join("manifest.json"))
            .ok()
            .is_some_and(|m| m["summary"].get("mechanism").is_some_and(|x| !x.is_null())) =>
        {
            let manifest = read_json(&dir.join("manifest.json")).map_err(|err| (OTHER, err))?;
            let summary = manifest.get("summary").cloned().unwrap_or(Value::Null);
            let feeds: Vec<crate::midas::FeedEntry> =
                serde_json::from_value(summary["feeds_configured"].clone()).map_err(|err| {
                    (
                        OTHER,
                        format!("the manifest summary carries no feeds_configured list: {err}"),
                    )
                })?;
            let mechanism: crate::midas::Mechanism =
                serde_json::from_value(summary["mechanism"].clone()).map_err(|err| {
                    (
                        OTHER,
                        format!("the manifest summary carries no mechanism: {err}"),
                    )
                })?;
            crate::run_midas::run(
                &mut source,
                crate::run_midas::RunArgs {
                    block,
                    target: summary["target"].as_str().unwrap_or(target).to_string(),
                    family: summary["family"]
                        .as_str()
                        .unwrap_or("midas-customfeed")
                        .to_string(),
                    explorer: summary["explorer"].clone(),
                    mechanism,
                    feeds,
                    feed_list_source: summary["feed_list_source"]
                        .as_str()
                        .unwrap_or("bundle manifest")
                        .to_string(),
                    stale_after_days: summary["stale_after_days"].as_u64().unwrap_or(30),
                    recent_days: summary["recent_days"].as_u64().unwrap_or(183),
                    trace: None,
                },
                replay_root,
            )
            .map(|outcome| outcome.result_path)
        }
        other => return Err((OTHER, format!("unknown target {other}"))),
    };
    match outcome {
        Ok(path) => Ok(path),
        Err(err) => {
            if let Some(miss) = source.missing().first() {
                Err((
                    BUNDLE_INCOMPLETE,
                    format!(
                        "the replay needed {} ({}) which the bundle does not hold; key {}",
                        miss.label, miss.method, miss.key
                    ),
                ))
            } else {
                Err((OTHER, format!("the replay failed: {err}")))
            }
        }
    }
}

/// The JSON-RPC entries a refetch samples: evenly spread over the manifest
/// in read order, so a small sample still touches both pinned blocks.
/// Blockscout and other HTTP GET bodies are excluded (their formatting is
/// not guaranteed stable).
pub fn sample_entries(entries: &[Value], sample: Sample) -> Vec<Value> {
    let json_rpc: Vec<&Value> = entries
        .iter()
        .filter(|e| e.get("wire").and_then(Value::as_str) == Some("json_rpc"))
        .collect();
    let picks: Vec<usize> = match sample {
        Sample::All => (0..json_rpc.len()).collect(),
        Sample::Count(n) => {
            let n = n.min(json_rpc.len());
            let mut picks: Vec<usize> = (0..n).map(|i| i * json_rpc.len() / n).collect();
            picks.dedup();
            picks
        }
    };
    picks.into_iter().map(|i| json_rpc[i].clone()).collect()
}

/// A manifest entry turned back into the read it records, so the same
/// request can be sent again.
fn descriptor_of(entry: &Value) -> Result<crate::rpc::Descriptor, String> {
    let text = |key: &str| -> Result<String, String> {
        entry
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("a manifest entry has no {key}: {entry}"))
    };
    Ok(crate::rpc::Descriptor {
        label: text("label")?,
        wire: crate::rpc::Wire::JsonRpc,
        method: text("method")?,
        block: text("block")?,
        to: text("to")?,
        calldata: text("calldata")?,
        params: entry
            .get("request")
            .and_then(|r| r.get("params"))
            .cloned()
            .unwrap_or(Value::Array(vec![])),
    })
}

/// Spec 03 R13: re-reads each sampled entry through `fetch` and compares
/// the parsed `result` with the bundle's. The comparison is on the JSON
/// value, not the bytes, because two nodes format one answer differently.
/// Returns the number of entries that agreed, or the exit code and detail
/// of the first that did not.
pub fn refetch_compare(
    dir: &Path,
    sampled: &[Value],
    mut fetch: impl FnMut(&crate::rpc::Descriptor) -> Result<String, String>,
) -> Result<usize, (u8, String)> {
    let mut agreed = 0usize;
    for entry in sampled {
        let descriptor = descriptor_of(entry).map_err(|err| (OTHER, err))?;
        let file = entry.get("file").and_then(Value::as_str).unwrap_or("?");
        let stored: Value = read_json(&dir.join(file)).map_err(|err| (OTHER, err))?;
        let refetched_body = fetch(&descriptor).map_err(|err| {
            (
                OTHER,
                format!("refetch of {} failed: {err}", descriptor.label),
            )
        })?;
        let refetched: Value = serde_json::from_str(&refetched_body).map_err(|err| {
            (
                OTHER,
                format!("refetch of {} is not JSON: {err}", descriptor.label),
            )
        })?;
        let stored_result = stored.get("result").cloned().unwrap_or(Value::Null);
        let refetched_result = refetched.get("result").cloned().unwrap_or(Value::Null);
        if stored_result != refetched_result {
            let short = |v: &Value| {
                let text = v.to_string();
                if text.len() > 120 {
                    format!("{}...", &text[..120])
                } else {
                    text
                }
            };
            return Err((
                REFETCH_MISMATCH,
                format!(
                    "{} ({} at {}): bundle {} but the endpoint now says {}",
                    descriptor.label,
                    descriptor.method,
                    descriptor.block,
                    short(&stored_result),
                    short(&refetched_result)
                ),
            ));
        }
        agreed += 1;
    }
    Ok(agreed)
}

/// The network step, after everything offline passed. A fresh empty cache
/// in the temporary root makes every sampled read a real network read.
fn refetch(
    dir: &Path,
    manifest: &Value,
    sample: Sample,
    endpoints: &[String],
    scratch: &Path,
) -> Result<(usize, usize), (u8, String)> {
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let sampled = sample_entries(&entries, sample);
    let chain_id = manifest
        .get("chain_id")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let endpoints = if endpoints.is_empty() {
        vec![
            crate::rpc::DEFAULT_ARCHIVE_ENDPOINT.to_string(),
            crate::rpc::DEFAULT_LATEST_ENDPOINT.to_string(),
        ]
    } else {
        endpoints.to_vec()
    };
    let mut client = crate::rpc::Client::new(
        endpoints,
        vec![],
        crate::cache::Cache::new(scratch.join("refetch-cache")),
        chain_id,
        false,
        0,
    );
    let agreed = refetch_compare(dir, &sampled, |descriptor| {
        client
            .fetch(descriptor.clone())
            .map(|fetched| fetched.body)
            .map_err(|err| err.message)
    })?;
    Ok((agreed, sampled.len()))
}

/// Verifies a bundle directory, or an archive written by `bundle pack`
/// (or any gzip'd tar holding one bundle directory): the archive is
/// unpacked into a temporary directory, verified like a directory, and the
/// report starts with the archive path and its sha256.
pub fn verify(path: &Path, options: &Options) -> Report {
    if !crate::pack::is_archive(path) {
        return verify_dir(path, options);
    }
    let mut report = Report::new();
    report.line("archive", path.display());
    let archive_sha256 = match fs::read(path) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(err) => return report.fail(OTHER, "UNREADABLE", format!("{}: {err}", path.display())),
    };
    report.line("archive sha256", &archive_sha256);
    static UNPACKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "crossfoot-unpack-{}-{}-{}",
        crate::util::now_stamp(),
        std::process::id(),
        UNPACKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let dir = match crate::pack::unpack(path, &scratch) {
        Ok(dir) => dir,
        Err(err) => {
            let _ = fs::remove_dir_all(&scratch);
            return report.fail(OTHER, "UNREADABLE", err);
        }
    };
    let inner = verify_dir(&dir, options);
    let _ = fs::remove_dir_all(&scratch);
    report.lines.extend(inner.lines);
    report.exit_code = inner.exit_code;
    report.status = inner.status;
    report
}

fn verify_dir(dir: &Path, options: &Options) -> Report {
    let require_same_code = options.require_same_code;
    let mut report = Report::new();
    report.line("bundle", dir.display());

    // (a) parse.
    let manifest = match read_json(&dir.join("manifest.json")) {
        Ok(manifest) => manifest,
        Err(err) => return report.fail(OTHER, "UNREADABLE", err),
    };
    let format = manifest.get("format").and_then(Value::as_str).unwrap_or("");
    if format != MANIFEST_FORMAT {
        return report.fail(
            OTHER,
            "UNSUPPORTED_FORMAT",
            format!("the manifest is {format:?}, this verifier reads {MANIFEST_FORMAT}"),
        );
    }
    let meta = read_json(&dir.join("meta.json")).ok();
    let result = match read_json(&dir.join("result.json")) {
        Ok(result) => Some(result),
        Err(_) if !dir.join("result.json").exists() => None,
        Err(err) => return report.fail(OTHER, "UNREADABLE", err),
    };
    let target = result
        .as_ref()
        .and_then(|r| r.get("target"))
        .and_then(Value::as_str)
        .map(str::to_string);
    match (&target, result.as_ref().and_then(|r| r.get("window"))) {
        (Some(target), Some(window)) => report.line(
            "target",
            format!(
                "{target}, window {} to {}",
                window
                    .get("baseline_block")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                window.get("block").and_then(Value::as_u64).unwrap_or(0)
            ),
        ),
        _ => report.line(
            "target",
            format!(
                "{} (fetch bundle, no result to replay)",
                manifest
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            ),
        ),
    }

    // (b) to (d).
    let root = match check_hashes(dir, &manifest, &mut report) {
        Ok(root) => root,
        Err(detail) => return report.fail(HASH_MISMATCH, "HASH_MISMATCH", detail),
    };
    report.line("root hash", &root);

    // (g), computed here so the replay verdict can be read next to it.
    let producer = manifest.get("code").cloned().unwrap_or(Value::Null);
    let verifier = crate::util::code_identity();
    let same_code = producer == verifier;
    let identity = |code: &Value| -> String {
        format!(
            "{} at {} (dirty: {}, packages {})",
            code.get("tool_version")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            code.get("git_commit")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            code.get("git_dirty")
                .map(|d| d.to_string())
                .unwrap_or("?".into()),
            code.get("packages_sha256")
                .and_then(Value::as_str)
                .map(|h| h.get(..12).unwrap_or(h))
                .unwrap_or("?")
        )
    };
    report.line("producer code", identity(&producer));
    report.line("verifier code", identity(&verifier));

    // (e) and (f).
    let (Some(result), Some(target)) = (result, target) else {
        report.line("replay", "NO_RESULT, hashes only");
        report.line("network", "none");
        report.status = "NO_RESULT";
        report.line(
            "status",
            "NO_RESULT: the bundle hashes check out and carries no result to replay",
        );
        report.line("scope", SCOPE_SENTENCE);
        return report;
    };
    // One replay root per verification, also when several run in one
    // process at once: stamp, process id and a counter.
    static REPLAYS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let replay_root = std::env::temp_dir().join(format!(
        "crossfoot-verify-{}-{}-{}",
        crate::util::now_stamp(),
        std::process::id(),
        REPLAYS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let replayed = replay(dir, &target, &result, meta.as_ref(), &replay_root);
    let outcome = match replayed {
        Ok(path) => {
            let ours = fs::read(&path).unwrap_or_default();
            let theirs = fs::read(dir.join("result.json")).unwrap_or_default();
            if ours == theirs {
                Ok(())
            } else {
                let replayed_value: Value = serde_json::from_slice(&ours).unwrap_or(Value::Null);
                let detail = match first_difference(&result, &replayed_value) {
                    Some((path, stated, computed)) => {
                        format!("result.json differs at {path}: bundle {stated}, replay {computed}")
                    }
                    None => "result.json differs in bytes only (formatting)".to_string(),
                };
                Err((REPLAY_MISMATCH, "REPLAY_MISMATCH", detail))
            }
        }
        Err((BUNDLE_INCOMPLETE, detail)) => Err((BUNDLE_INCOMPLETE, "BUNDLE_INCOMPLETE", detail)),
        Err((_, detail)) => Err((OTHER, "REPLAY_FAILED", detail)),
    };
    // (h) the optional refetch, after everything offline passed.
    let refetched = match (&outcome, options.refetch) {
        (Ok(()), Some(sample)) => Some(refetch(
            dir,
            &manifest,
            sample,
            &options.endpoints,
            &replay_root,
        )),
        _ => None,
    };
    let _ = fs::remove_dir_all(&replay_root);
    match &refetched {
        None => report.line("network", "none"),
        Some(Ok((agreed, sampled))) => report.line(
            "network",
            format!("refetched {agreed} of {sampled} sampled JSON-RPC entries, all agree"),
        ),
        Some(Err(_)) => report.line("network", "refetch"),
    }

    if !same_code {
        report.line(
            "warning",
            "the bundle was produced by different code; a replay mismatch may come from that rather than from the responses",
        );
    }
    match outcome {
        Ok(()) => {
            report.line("replay", "result.json reproduced byte for byte");
            if let Some(Err((code, detail))) = refetched {
                let status = if code == REFETCH_MISMATCH {
                    "REFETCH_MISMATCH"
                } else {
                    "REFETCH_FAILED"
                };
                return report.fail(code, status, detail);
            }
            if require_same_code && !same_code {
                return report.fail(
                    CODE_MISMATCH,
                    "CODE_MISMATCH",
                    "the producer's code identity differs from this verifier's and --require-same-code was given",
                );
            }
            report.line("status", "VERIFIED");
            report.line("scope", SCOPE_SENTENCE);
            report
        }
        Err((code, status, detail)) => {
            report.line("replay", status);
            report.fail(code, status, detail)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::seal;
    use serde_json::json;
    use std::str::FromStr;

    /// The svZCHF demo window bundle, produced by `crossfoot run svzchf
    /// --window demo` and checked in (spec 01 R2, spec 03 fixtures).
    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("svzchf-demo-24570000-25853000")
    }

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// A scratch copy of the fixture to tamper with.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crossfoot-verify-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        copy_dir(&fixture(), &dir);
        dir
    }

    fn print(report: &Report) -> String {
        report.lines.join("\n")
    }

    #[test]
    fn verify_passes_on_an_untouched_bundle() {
        let report = verify(&fixture(), &Options::default());
        assert_eq!(report.exit_code, VERIFIED, "{}", print(&report));
        assert_eq!(report.status, "VERIFIED");
        let text = print(&report);
        assert!(
            text.contains("entries         32 checked, hashes ok"),
            "{text}"
        );
        assert!(
            text.contains("result.json reproduced byte for byte"),
            "{text}"
        );
        let stated = fs::read_to_string(fixture().join("bundle.sha256")).unwrap();
        assert!(
            text.contains(&format!("root hash       {}", stated.trim())),
            "{text}"
        );
    }

    #[test]
    fn verify_detects_one_flipped_byte_in_raw() {
        let dir = scratch("flipped");
        let file = dir.join("raw").join("003-vault-asset.json");
        let mut bytes = fs::read(&file).unwrap();
        // Flip a byte inside the result hex, keeping the length.
        let at = bytes.len() - 5;
        bytes[at] = if bytes[at] == b'0' { b'1' } else { b'0' };
        fs::write(&file, bytes).unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(print(&report).contains("raw/003-vault-asset.json: sha256 differs"));
    }

    #[test]
    fn verify_detects_a_missing_raw_file() {
        let dir = scratch("missing");
        fs::remove_file(dir.join("raw").join("004-vault-savings.json")).unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(print(&report).contains("raw/004-vault-savings.json"));
        // An extra file is a change as well.
        let dir = scratch("extra");
        fs::write(dir.join("raw").join("999-extra.json"), "{}").unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(print(&report).contains("raw/999-extra.json: present but not in the manifest"));
    }

    /// Spec 03 R9: a tampered result whose hashes were re-sealed still fails,
    /// and the first differing JSON path is printed with both values.
    #[test]
    fn verify_detects_a_tampered_result() {
        let dir = scratch("tampered");
        let mut result: Value = read_json(&dir.join("result.json")).unwrap();
        result["comparison"]["fields"][3]["observed"] = json!("1021764268673581425");
        let mut text = serde_json::to_string_pretty(&result).unwrap();
        text.push('\n');
        fs::write(dir.join("result.json"), text).unwrap();
        seal(&dir).unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, REPLAY_MISMATCH, "{}", print(&report));
        let text = print(&report);
        assert!(
            text.contains("result.json differs at comparison.fields[3].observed: bundle \"1021764268673581425\", replay \"1021764268673581424\""),
            "{text}"
        );
    }

    #[test]
    fn verify_prints_the_first_differing_json_path() {
        let a = json!({"a": 1, "b": [1, {"c": "x"}], "z": null});
        let b = json!({"a": 1, "b": [1, {"c": "y"}], "z": null});
        assert_eq!(
            first_difference(&a, &b),
            Some(("b[1].c".to_string(), json!("x"), json!("y")))
        );
        let missing = json!({"a": 1});
        assert_eq!(
            first_difference(&a, &missing),
            Some(("b".to_string(), json!([1, {"c": "x"}]), Value::Null))
        );
        assert_eq!(first_difference(&a, &a), None);
    }

    /// Spec 03 R8 (e): the replay needs a read the bundle no longer holds.
    #[test]
    fn verify_reports_a_bundle_with_a_removed_entry_as_incomplete() {
        let dir = scratch("removed");
        let mut manifest: Value = read_json(&dir.join("manifest.json")).unwrap();
        let entries = manifest["entries"].as_array_mut().unwrap();
        // The B0 account read is what the replay seeds from.
        let index = entries
            .iter()
            .position(|e| e["file"] == "raw/028-module-savings-vault.json")
            .expect("the fixture holds the baseline account read");
        entries.remove(index);
        manifest["entry_count"] = json!(entries.len());
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
        fs::remove_file(dir.join("raw").join("028-module-savings-vault.json")).unwrap();
        seal(&dir).unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, BUNDLE_INCOMPLETE, "{}", print(&report));
        let text = print(&report);
        assert!(text.contains("module.savings(vault)"), "{text}");
        assert!(text.contains("key "), "{text}");
    }

    /// Spec 03 R10: a code difference is a warning, and exit 5 only when
    /// the same code is required.
    #[test]
    fn verify_code_mismatch_is_a_warning_unless_required() {
        let dir = scratch("code");
        let mut manifest: Value = read_json(&dir.join("manifest.json")).unwrap();
        manifest["code"]["git_commit"] = json!("0000000000000000000000000000000000000000");
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
        seal(&dir).unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, VERIFIED, "{}", print(&report));
        assert!(
            print(&report).contains("warning         the bundle was produced by different code")
        );
        let report = verify(
            &dir,
            &Options {
                require_same_code: true,
                ..Options::default()
            },
        );
        assert_eq!(report.exit_code, CODE_MISMATCH, "{}", print(&report));
        assert_eq!(report.status, "CODE_MISMATCH");
    }

    /// Spec 03 R11: the verifier constructs no client. The bundle source is
    /// the only read path, so the exit code cannot depend on the network;
    /// the report says so on its own line.
    #[test]
    fn verify_makes_no_network_call() {
        let report = verify(&fixture(), &Options::default());
        assert_eq!(report.exit_code, VERIFIED, "{}", print(&report));
        assert!(print(&report).contains("network         none"));
    }

    #[test]
    fn verify_report_carries_the_scope_sentence() {
        let report = verify(&fixture(), &Options::default());
        let text = print(&report);
        assert!(text.contains(SCOPE_SENTENCE), "{text}");
        for key in [
            "bundle",
            "target",
            "entries",
            "root hash",
            "replay",
            "producer code",
            "verifier code",
            "status",
        ] {
            assert!(
                text.lines().any(|l| l.starts_with(key)),
                "no {key} line in {text}"
            );
        }
    }

    /// Spec 03 R7: the replay window comes from the bundle's result.json,
    /// not from anything in the working tree or on the command line.
    #[test]
    fn replay_takes_window_and_feeds_from_the_bundle_not_the_tree() {
        let report = verify(&fixture(), &Options::default());
        assert!(
            print(&report).contains("target          svzchf, window 24570000 to 25853000"),
            "{}",
            print(&report)
        );
    }

    /// Spec 03 R13, offline: the sample is spread over the JSON-RPC entries,
    /// a re-read that agrees as a JSON value passes whatever its bytes, and
    /// a re-read that disagrees is REFETCH_MISMATCH naming the read.
    #[test]
    fn refetch_reports_a_mismatch_between_bundle_and_endpoint() {
        let manifest: Value = read_json(&fixture().join("manifest.json")).unwrap();
        let entries = manifest["entries"].as_array().unwrap().clone();
        let json_rpc = entries.iter().filter(|e| e["wire"] == "json_rpc").count();
        assert_eq!(
            json_rpc, 26,
            "the fixture holds 26 JSON-RPC reads and 6 Blockscout reads"
        );
        let all = sample_entries(&entries, Sample::All);
        assert_eq!(all.len(), 26);
        let three = sample_entries(&entries, Sample::Count(3));
        assert_eq!(three.len(), 3);
        // Spread: one from the B1 fetch (entries 1 to 16), one across the
        // middle, one from the B0 fetch.
        assert_eq!(three[0]["index"], 1);
        assert!(three[2]["index"].as_u64().unwrap() > 16);
        assert!(three.iter().all(|e| e["wire"] == "json_rpc"));
        let many = sample_entries(&entries, Sample::Count(1000));
        assert_eq!(
            many.len(),
            26,
            "a count above the population is the population"
        );

        // A mock endpoint that answers from the bundle's own bodies but
        // reformats them: agreement is on the JSON value.
        let reformat = |descriptor: &crate::rpc::Descriptor| -> Result<String, String> {
            let entry = entries
                .iter()
                .find(|e| {
                    e["label"] == descriptor.label.as_str()
                        && e["block"] == descriptor.block.as_str()
                })
                .unwrap();
            let body = read_json(&fixture().join(entry["file"].as_str().unwrap())).unwrap();
            Ok(serde_json::to_string_pretty(
                &json!({"jsonrpc": "2.0", "id": 7, "result": body["result"]}),
            )
            .unwrap())
        };
        assert_eq!(refetch_compare(&fixture(), &all, reformat).unwrap(), 26);

        // The same endpoint, lying about one pinned read.
        let lying = |descriptor: &crate::rpc::Descriptor| -> Result<String, String> {
            if descriptor.label == "vault.price()" && descriptor.block == "0x18a7c48" {
                return Ok(r#"{"jsonrpc":"2.0","id":1,"result":"0x00"}"#.to_string());
            }
            reformat(descriptor)
        };
        let (code, detail) = refetch_compare(&fixture(), &all, lying).unwrap_err();
        assert_eq!(code, REFETCH_MISMATCH);
        assert!(
            detail.starts_with("vault.price() (eth_call at 0x18a7c48): bundle"),
            "{detail}"
        );
        assert!(
            detail.contains("the endpoint now says \"0x00\""),
            "{detail}"
        );

        // A network failure is exit 1, not a mismatch.
        let down = |_: &crate::rpc::Descriptor| -> Result<String, String> { Err("refused".into()) };
        let (code, detail) = refetch_compare(&fixture(), &three, down).unwrap_err();
        assert_eq!(code, OTHER);
        assert!(
            detail.contains("refetch of eth_chainId failed: refused"),
            "{detail}"
        );

        // The verify entry point does not refetch when the offline steps
        // failed, so a tampered bundle stays exit 2 whatever the flag.
        let dir = scratch("refetch-tampered");
        fs::remove_file(dir.join("raw").join("004-vault-savings.json")).unwrap();
        let report = verify(
            &dir,
            &Options {
                refetch: Some(Sample::Count(1)),
                endpoints: vec!["http://127.0.0.1:9".to_string()],
                ..Options::default()
            },
        );
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(Sample::from_str("all").is_ok());
        assert_eq!(Sample::from_str("3"), Ok(Sample::Count(3)));
        assert!(Sample::from_str("0").is_err());
        assert!(Sample::from_str("some").is_err());
    }

    /// The Midas family fixture (spec 02 R19) verifies: 1,812 entries, the
    /// replay through the midas arm reproduces result.json byte for byte
    /// with the feed list taken from the manifest, and the window line
    /// names the survey block.
    #[test]
    fn verify_passes_on_the_midas_fixture() {
        let fixture = crate::fixtures::midas_bundle();
        let report = verify(&fixture, &Options::default());
        let text = print(&report);
        assert_eq!(report.exit_code, VERIFIED, "{text}");
        assert!(
            text.contains("target          midas, window 0 to 25884405"),
            "{text}"
        );
        assert!(
            text.contains("entries         1812 checked, hashes ok"),
            "{text}"
        );
        assert!(
            text.contains("result.json reproduced byte for byte"),
            "{text}"
        );
    }

    /// A timeline file is not a manifest entry, so a change to it is caught
    /// by the checksum list rather than by the entry hashes.
    #[test]
    fn verify_detects_a_tampered_timeline() {
        let dir = std::env::temp_dir().join("crossfoot-verify-midas-timeline");
        let _ = fs::remove_dir_all(&dir);
        copy_dir(&crate::fixtures::midas_bundle(), &dir);
        let timeline = fs::read_dir(dir.join("timelines"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("mre7"))
            })
            .expect("the mRE7 timeline is in the fixture");
        let mut text = fs::read_to_string(&timeline).unwrap();
        text.push(' ');
        fs::write(&timeline, text).unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(
            print(&report).contains("SHA256SUMS differs from the files at timelines/"),
            "{}",
            print(&report)
        );
    }

    /// A packed archive verifies like the directory it holds, the report
    /// leads with the archive's sha256, and an archive repacked after a
    /// change to one raw body is HASH_MISMATCH naming the file.
    #[test]
    fn verify_accepts_a_packed_archive_and_detects_a_tampered_one() {
        let dir =
            std::env::temp_dir().join(format!("crossfoot-verify-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let packed = crate::pack::pack(&fixture(), &dir.join("demo.tar.gz")).unwrap();
        let report = verify(&packed.archive, &Options::default());
        let text = print(&report);
        assert_eq!(report.exit_code, VERIFIED, "{text}");
        assert!(
            text.starts_with(&format!("archive         {}", packed.archive.display())),
            "{text}"
        );
        assert!(
            text.contains(&format!("archive sha256  {}", packed.archive_sha256)),
            "{text}"
        );
        assert!(
            text.contains("result.json reproduced byte for byte"),
            "{text}"
        );
        assert!(
            text.contains(&format!("root hash       {}", packed.root_hash.unwrap())),
            "{text}"
        );

        // Unpack, change one byte, pack again: the hashes catch it.
        let unpacked = crate::pack::unpack(&packed.archive, &dir.join("unpacked")).unwrap();
        let file = unpacked.join("raw").join("003-vault-asset.json");
        let mut bytes = fs::read(&file).unwrap();
        let at = bytes.len() - 5;
        bytes[at] = if bytes[at] == b'0' { b'1' } else { b'0' };
        fs::write(&file, bytes).unwrap();
        let tampered = crate::pack::pack(&unpacked, &dir.join("tampered.tar.gz")).unwrap();
        assert_ne!(tampered.archive_sha256, packed.archive_sha256);
        let report = verify(&tampered.archive, &Options::default());
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(print(&report).contains("raw/003-vault-asset.json: sha256 differs"));

        // A file that is not an archive is verified as a directory would be.
        let report = verify(&dir.join("missing.tar.gz"), &Options::default());
        assert_eq!(report.exit_code, OTHER);
    }

    /// The Midas fixture archive verifies as checked in, without extracting
    /// it by hand.
    #[test]
    fn verify_accepts_the_midas_archive_directly() {
        let archive = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("midas-25884405.tar.gz");
        let report = verify(&archive, &Options::default());
        let text = print(&report);
        assert_eq!(report.exit_code, VERIFIED, "{text}");
        assert!(
            text.contains("entries         1812 checked, hashes ok"),
            "{text}"
        );
    }

    /// The README claims exactly what the verifier proves, in the same
    /// words.
    #[test]
    fn readme_claim_matches_the_scope_sentence() {
        let readme = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("README.md"),
        )
        .unwrap();
        // The README wraps lines; compare with whitespace collapsed.
        let collapse = |text: &str| text.split_whitespace().collect::<Vec<&str>>().join(" ");
        assert!(
            collapse(&readme).contains(&collapse(SCOPE_SENTENCE)),
            "the README does not carry the scope sentence verbatim"
        );
    }

    #[test]
    fn verify_refuses_an_older_manifest_format() {
        let dir = scratch("v1");
        let mut manifest: Value = read_json(&dir.join("manifest.json")).unwrap();
        manifest["format"] = json!("crossfoot-manifest-v1");
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
        let report = verify(&dir, &Options::default());
        assert_eq!(report.exit_code, OTHER, "{}", print(&report));
        assert!(print(&report).contains("crossfoot-manifest-v1"));
    }
}
