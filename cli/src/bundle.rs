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
use crate::cache::sha256_hex;
use crate::rpc::Fetched;
use crate::util::slug;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub index: usize,
    pub file: String,
    pub sha256: String,
    pub byte_len: usize,
    pub label: String,
    pub method: String,
    /// Pinned block, or the inclusive range for eth_getLogs.
    pub block: String,
    pub to: String,
    pub calldata: String,
    pub request: Value,
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
    entries: Vec<Entry>,
    findings: Vec<Finding>,
}

impl BundleWriter {
    /// Creates a fresh bundle directory. The name carries a timestamp only to
    /// second resolution, so two runs started in the same second would
    /// otherwise write into one directory and overwrite each other. A numeric
    /// suffix keeps every run's evidence separate.
    pub fn create(bundles_root: &Path, name: &str) -> io::Result<Self> {
        let mut dir = bundles_root.join(name);
        let mut attempt = 2;
        while dir.exists() {
            dir = bundles_root.join(format!("{name}-{attempt}"));
            attempt += 1;
            if attempt > 1000 {
                return Err(io::Error::other(format!(
                    "could not find a free bundle directory name for {name}"
                )));
            }
        }
        let raw_dir = dir.join("raw");
        fs::create_dir_all(&raw_dir)?;
        Ok(Self {
            dir,
            raw_dir,
            entries: Vec::new(),
            findings: Vec::new(),
        })
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
            method: fetched.descriptor.method.clone(),
            block: fetched.descriptor.block.clone(),
            to: fetched.descriptor.to.clone(),
            calldata: fetched.descriptor.calldata.clone(),
            request: fetched.descriptor.request_body(),
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
            "format": "crossfoot-manifest-v1",
            "target": target,
            "bundle": self.dir.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
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
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    text.push('\n');
    fs::write(path, text.as_bytes())
}
