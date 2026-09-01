//! Evidence bundle writer.
//!
//! A bundle is a directory holding every raw JSON-RPC response that went into
//! a run, verbatim, plus a manifest that hashes each one and states which
//! request produced it, and a meta file describing the code and the pinned
//! inputs. Nothing in raw/ is rewritten, reformatted or normalised: a third
//! party can hash the files and compare against the manifest without trusting
//! this tool.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::abi::Decoded;
use crate::cache::{key_preimage, sha256_hex, PREIMAGE_VERSION};
use crate::rpc::{Fetched, Wire};
use crate::util::slug;

pub const MANIFEST_FORMAT: &str = "crossfoot-manifest-v2";

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub index: usize,
    pub file: String,
    pub sha256: String,
    pub byte_len: usize,
    pub label: String,
    /// "json_rpc" or "http_get".
    pub wire: &'static str,
    pub method: String,
    /// Pinned block, or the inclusive range for eth_getLogs.
    pub block: String,
    pub to: String,
    pub calldata: String,
    pub request: Value,
    /// The exact cache key preimage, so a reader recomputes the key without
    /// this code: sha256 over these bytes is `cache_key`.
    pub preimage: String,
    pub cache_key: String,
    /// "hit" or "miss" for this run.
    pub cache: &'static str,
    /// The endpoint that originally produced this body, carried through cache
    /// hits from the cache metadata.
    pub endpoint: String,
    pub first_stored_utc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoded: Option<Decoded>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finding: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub kind: String,
    pub label: String,
    pub detail: String,
}

pub struct BundleWriter {
    dir: PathBuf,
    raw_dir: PathBuf,
    chain_id: u64,
    entries: Vec<Entry>,
    findings: Vec<Finding>,
}

impl BundleWriter {
    /// Creates a fresh bundle directory. The name carries a timestamp only to
    /// second resolution, so two runs started in the same second would
    /// otherwise write into one directory and overwrite each other. A numeric
    /// suffix keeps every run's evidence separate.
    pub fn create(bundles_root: &Path, name: &str, chain_id: u64) -> io::Result<Self> {
        fs::create_dir_all(bundles_root)?;
        // Claiming the directory is the check: create_dir fails when it
        // exists, so two runs racing for one name cannot both win it.
        let mut dir = bundles_root.join(name);
        let mut attempt = 2;
        loop {
            match fs::create_dir(&dir) {
                Ok(()) => break,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    dir = bundles_root.join(format!("{name}-{attempt}"));
                    attempt += 1;
                    if attempt > 1000 {
                        return Err(io::Error::other(format!(
                            "could not find a free bundle directory name for {name}"
                        )));
                    }
                }
                Err(err) => return Err(err),
            }
        }
        let raw_dir = dir.join("raw");
        fs::create_dir_all(&raw_dir)?;
        Ok(Self {
            dir,
            raw_dir,
            chain_id,
            entries: Vec::new(),
            findings: Vec::new(),
        })
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    pub fn add_finding(&mut self, kind: &str, label: &str, detail: impl Into<String>) {
        self.findings.push(Finding {
            kind: kind.to_string(),
            label: label.to_string(),
            detail: detail.into(),
        });
    }

    /// Writes one raw response and records its manifest entry. The file name
    /// is derived from the read order and the label, both of which are fixed
    /// by the fetch plan, so two runs at the same block produce the same set
    /// of file names.
    pub fn record(
        &mut self,
        fetched: &Fetched,
        decoded: Option<Decoded>,
        finding: Option<String>,
    ) -> io::Result<()> {
        let index = self.entries.len() + 1;
        let file_name = format!("{index:03}-{}.json", slug(&fetched.descriptor.label));
        fs::write(self.raw_dir.join(&file_name), fetched.body.as_bytes())?;
        self.entries.push(Entry {
            index,
            file: format!("raw/{file_name}"),
            sha256: sha256_hex(fetched.body.as_bytes()),
            byte_len: fetched.body.len(),
            label: fetched.descriptor.label.clone(),
            wire: match fetched.descriptor.wire {
                Wire::JsonRpc => "json_rpc",
                Wire::HttpGet { .. } => "http_get",
            },
            method: fetched.descriptor.method.clone(),
            block: fetched.descriptor.block.clone(),
            to: fetched.descriptor.to.clone(),
            calldata: fetched.descriptor.calldata.clone(),
            request: fetched.descriptor.request_body(),
            preimage: key_preimage(self.chain_id, &fetched.descriptor),
            cache_key: fetched.key.clone(),
            cache: if fetched.cache_hit { "hit" } else { "miss" },
            endpoint: fetched.endpoint.clone(),
            first_stored_utc: fetched.stored_utc.clone(),
            decoded,
            finding,
        });
        Ok(())
    }

    pub fn write_manifest(&self, target: &str, extra: Value) -> io::Result<()> {
        let manifest = serde_json::json!({
            "format": MANIFEST_FORMAT,
            "target": target,
            "bundle": self.dir.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
            "chain_id": self.chain_id,
            "cache_preimage_version": PREIMAGE_VERSION,
            "code": crate::util::code_identity(),
            "entry_count": self.entries.len(),
            "cache_hits": self.entries.iter().filter(|e| e.cache == "hit").count(),
            "cache_misses": self.entries.iter().filter(|e| e.cache == "miss").count(),
            "entries": self.entries,
            "findings": self.findings,
            "summary": extra,
        });
        write_json(&self.dir.join("manifest.json"), &manifest)
    }

    pub fn write_meta(&self, meta: Value) -> io::Result<()> {
        write_json(&self.dir.join("meta.json"), &meta)
    }

    /// Writes result.json. The result has to be a pure function of the raw
    /// bodies and the code (spec 01 R8, spec 03 R4), so a result that carries
    /// a run-time field is refused here rather than written: wall-clock
    /// timings, cache and network counters, endpoint names and references to
    /// other bundles belong in meta.json.
    pub fn write_result(&self, result: &Value) -> Result<PathBuf, String> {
        if let Some(path) = impure_result_field(result) {
            return Err(format!(
                "result.json must not carry the run-time field {path}; it belongs in meta.json"
            ));
        }
        let path = self.dir.join("result.json");
        write_json(&path, result).map_err(|err| format!("could not write result.json: {err}"))?;
        Ok(path)
    }
}

/// Keys that describe the run rather than the result. Chain timestamps such
/// as `timestamp_utc` or `last_post_utc` are properties of the inputs and
/// stay allowed; only the tool's own clock, counters and endpoints are not.
const IMPURE_RESULT_KEYS: [&str; 15] = [
    "run_started_utc",
    "run_finished_utc",
    "fetch_started_utc",
    "fetch_finished_utc",
    "first_stored_utc",
    "stored_utc",
    "endpoint",
    "endpoints_configured",
    "log_endpoints_configured",
    "endpoint_fingerprints",
    "cache_hits_this_run",
    "network_calls_this_run",
    "rpc_observations",
    "b0_bundle",
    "b1_bundle",
];

/// The JSON path of the first run-time field in a result, or None when the
/// result is pure. Walks every object and array.
pub fn impure_result_field(value: &Value) -> Option<String> {
    fn walk(value: &Value, path: &str) -> Option<String> {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let here = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    if IMPURE_RESULT_KEYS.contains(&key.as_str()) {
                        return Some(here);
                    }
                    if let Some(found) = walk(child, &here) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(items) => items
                .iter()
                .enumerate()
                .find_map(|(index, item)| walk(item, &format!("{path}[{index}]"))),
            _ => None,
        }
    }
    walk(value, "")
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    text.push('\n');
    fs::write(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::cache_key;
    use crate::rpc::{
        blockscout_logs_descriptor, call_descriptor, chain_id_descriptor, Descriptor,
    };
    use serde_json::json;

    fn fetched(chain_id: u64, descriptor: Descriptor, body: &str) -> Fetched {
        Fetched {
            key: cache_key(chain_id, &descriptor),
            descriptor,
            body: body.to_string(),
            cache_hit: false,
            endpoint: "https://rpc.example/v1/<redacted>".to_string(),
            stored_utc: "2026-01-01T00:00:00.000Z".to_string(),
        }
    }

    /// Spec 03 R2: every entry's preimage hashes to its cache key, the
    /// header names the chain, the preimage version and the code identity.
    #[test]
    fn manifest_v2_preimage_recomputes_the_cache_key() {
        let root = std::env::temp_dir().join("crossfoot-manifest-v2");
        let _ = fs::remove_dir_all(&root);
        let mut writer = BundleWriter::create(&root, "svzchf-run-1-2-stamp", 1).unwrap();
        let reads = [
            fetched(
                1,
                chain_id_descriptor(),
                r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#,
            ),
            fetched(
                1,
                call_descriptor("vault.price()", "0xAbC", "0xA035B1FE", "0x2"),
                r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#,
            ),
            fetched(
                1,
                blockscout_logs_descriptor("rate history", "0xAbC", Some("0xD76D"), None, 0, 2),
                r#"{"message":"OK","result":[],"status":"1"}"#,
            ),
        ];
        for read in &reads {
            writer.record(read, None, None).unwrap();
        }
        writer.write_manifest("svzchf-run", json!({})).unwrap();

        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(writer.dir().join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["format"], "crossfoot-manifest-v2");
        assert_eq!(manifest["chain_id"], 1);
        assert_eq!(manifest["cache_preimage_version"], "crossfoot-cache-v1");
        assert_eq!(manifest["code"], crate::util::code_identity());
        let entries = manifest["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        for entry in entries {
            let preimage = entry["preimage"].as_str().unwrap();
            assert_eq!(
                sha256_hex(preimage.as_bytes()),
                entry["cache_key"].as_str().unwrap(),
                "entry {}",
                entry["file"]
            );
            assert!(preimage.starts_with("crossfoot-cache-v1\nchain_id=1\n"));
            let body = fs::read(writer.dir().join(entry["file"].as_str().unwrap())).unwrap();
            assert_eq!(sha256_hex(&body), entry["sha256"].as_str().unwrap());
            assert_eq!(body.len() as u64, entry["byte_len"].as_u64().unwrap());
        }
        assert_eq!(entries[0]["wire"], "json_rpc");
        assert_eq!(entries[2]["wire"], "http_get");
    }

    /// Two writers asking for one name in the same second get two
    /// directories, whichever order the file system answers in.
    #[test]
    fn two_bundles_never_share_a_directory() {
        let root = std::env::temp_dir().join("crossfoot-bundle-names");
        let _ = fs::remove_dir_all(&root);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let root = root.clone();
                std::thread::spawn(move || {
                    BundleWriter::create(&root, "run-1-2-stamp", 1)
                        .unwrap()
                        .dir()
                        .to_path_buf()
                })
            })
            .collect();
        let mut dirs: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        dirs.sort();
        dirs.dedup();
        assert_eq!(dirs.len(), 8, "every writer got a directory of its own");
    }

    /// Spec 01 R8, spec 03 R4: a schema walk over result-shaped values. The
    /// same walk guards every real result at write time.
    #[test]
    fn result_json_has_no_timing_or_endpoint_fields() {
        // Result-shaped values for the three targets, carrying the chain
        // timestamps that are allowed.
        let svzchf = json!({
            "format": "crossfoot-result-v1", "target": "svzchf", "verdict": "MODEL_MATCH",
            "summary": {"target": "svzchf", "window": {"baseline_block": 1, "block": 2}},
            "window": {"baseline_block": 1, "baseline_timestamp_unix": 10, "block": 2, "block_timestamp_unix": 20},
            "inputs": {"rate_segments": [{"start": 5, "rate_ppm": 30000}]},
            "replay_steps": [{"block": 1, "timestamp": 11, "action": "save"}],
        });
        let mtbill = json!({
            "format": "crossfoot-result-v1", "target": "mtbill", "consistency": "CONSISTENT",
            "benchmark": {"source": "home.treasury.gov", "pinning": "timestamp pinned"},
            "posting_eras": [{"from_utc": "2024-08-21T00:00:00Z"}],
        });
        let midas = json!({
            "format": "crossfoot-result-v1", "target": "midas",
            "feeds": [{"product": "mRE7", "last_post_utc": "2026-05-06T19:03:00Z", "findings": [{"timestamp_unix": 1}]}],
        });
        for result in [&svzchf, &mtbill, &midas] {
            assert_eq!(impure_result_field(result), None, "{result}");
        }

        // Every run-time key is caught wherever it sits, with its path.
        for key in IMPURE_RESULT_KEYS {
            let top = json!({ key: 1 });
            assert_eq!(impure_result_field(&top).as_deref(), Some(key));
            let nested = json!({ "inputs": { "detail": [ { key: "x" } ] } });
            assert_eq!(
                impure_result_field(&nested),
                Some(format!("inputs.detail[0].{key}"))
            );
        }

        // And the writer refuses such a result instead of writing it.
        let dir = std::env::temp_dir().join("crossfoot-bundle-purity");
        let _ = fs::remove_dir_all(&dir);
        let writer = BundleWriter::create(&dir, "run", 1).unwrap();
        let err = writer
            .write_result(&json!({ "verdict": "MODEL_MATCH", "run_started_utc": "now" }))
            .unwrap_err();
        assert!(err.contains("run_started_utc"), "{err}");
        assert!(!writer.dir().join("result.json").exists());
        let path = writer.write_result(&svzchf).unwrap();
        assert!(path.exists());
    }
}
