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

pub fn verify(dir: &Path, require_same_code: bool) -> Report {
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
    let _ = fs::remove_dir_all(&replay_root);
    report.line("network", "none");

    if !same_code {
        report.line(
            "warning",
            "the bundle was produced by different code; a replay mismatch may come from that rather than from the responses",
        );
    }
    match outcome {
        Ok(()) => {
            report.line("replay", "result.json reproduced byte for byte");
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
        let report = verify(&fixture(), false);
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
        let report = verify(&dir, false);
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(print(&report).contains("raw/003-vault-asset.json: sha256 differs"));
    }

    #[test]
    fn verify_detects_a_missing_raw_file() {
        let dir = scratch("missing");
        fs::remove_file(dir.join("raw").join("004-vault-savings.json")).unwrap();
        let report = verify(&dir, false);
        assert_eq!(report.exit_code, HASH_MISMATCH, "{}", print(&report));
        assert!(print(&report).contains("raw/004-vault-savings.json"));
        // An extra file is a change as well.
        let dir = scratch("extra");
        fs::write(dir.join("raw").join("999-extra.json"), "{}").unwrap();
        let report = verify(&dir, false);
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
        let report = verify(&dir, false);
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
        let report = verify(&dir, false);
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
        let report = verify(&dir, false);
        assert_eq!(report.exit_code, VERIFIED, "{}", print(&report));
        assert!(
            print(&report).contains("warning         the bundle was produced by different code")
        );
        let report = verify(&dir, true);
        assert_eq!(report.exit_code, CODE_MISMATCH, "{}", print(&report));
        assert_eq!(report.status, "CODE_MISMATCH");
    }

    /// Spec 03 R11: the verifier constructs no client. The bundle source is
    /// the only read path, so the exit code cannot depend on the network;
    /// the report says so on its own line.
    #[test]
    fn verify_makes_no_network_call() {
        let report = verify(&fixture(), false);
        assert_eq!(report.exit_code, VERIFIED, "{}", print(&report));
        assert!(print(&report).contains("network         none"));
    }

    #[test]
    fn verify_report_carries_the_scope_sentence() {
        let report = verify(&fixture(), false);
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
        let report = verify(&fixture(), false);
        assert!(
            print(&report).contains("target          svzchf, window 24570000 to 25853000"),
            "{}",
            print(&report)
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
        let report = verify(&dir, false);
        assert_eq!(report.exit_code, OTHER, "{}", print(&report));
        assert!(print(&report).contains("crossfoot-manifest-v1"));
    }
}
