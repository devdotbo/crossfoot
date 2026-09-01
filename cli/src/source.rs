//! A read source backed by an evidence bundle.
//!
//! `BundleSource` answers the same reads a network `Client` would, from the
//! verbatim bodies a bundle already holds, indexed by cache key. It owns no
//! HTTP agent and never opens a socket: a read whose key the bundle does not
//! hold is an `OfflineMiss`, recorded so a verifier can name what was
//! missing. This is what lets `crossfoot verify` recompute a result without
//! the network and without the producer's cache (spec 03 R6).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::cache::cache_key;
use crate::rpc::{Descriptor, Fetched, ReadSource, RpcError, RpcErrorKind};
use crate::util::parse_hex_u64;

/// What the manifest says about one raw body.
#[derive(Debug, Clone)]
struct Held {
    file: String,
    endpoint: String,
    first_stored_utc: String,
}

/// A read the bundle could not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    pub label: String,
    pub method: String,
    pub key: String,
}

pub struct BundleSource {
    dir: PathBuf,
    chain_id: u64,
    by_key: HashMap<String, Held>,
    served: usize,
    missing: Vec<Miss>,
}

impl BundleSource {
    /// Loads the manifest and indexes its entries by cache key. The chain id
    /// comes from the manifest header when it carries one, else from the
    /// bundle's own eth_chainId body, so an older manifest still replays.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let manifest_path = dir.join("manifest.json");
        let manifest: Value = serde_json::from_str(
            &fs::read_to_string(&manifest_path)
                .map_err(|err| format!("could not read {}: {err}", manifest_path.display()))?,
        )
        .map_err(|err| format!("{} is not JSON: {err}", manifest_path.display()))?;
        let entries = manifest
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{} has no entries", manifest_path.display()))?;

        let mut by_key = HashMap::new();
        let mut chain_id_entry: Option<String> = None;
        for entry in entries {
            let field = |name: &str| -> Result<String, String> {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("a manifest entry has no {name}: {entry}"))
            };
            let file = field("file")?;
            if entry.get("method").and_then(Value::as_str) == Some("eth_chainId") {
                chain_id_entry = Some(file.clone());
            }
            // Two entries with one key hold one body (the run read it twice);
            // the first one wins and the second is the same bytes.
            by_key.entry(field("cache_key")?).or_insert(Held {
                file,
                endpoint: field("endpoint").unwrap_or_default(),
                first_stored_utc: field("first_stored_utc").unwrap_or_default(),
            });
        }

        let chain_id = match manifest.get("chain_id").and_then(Value::as_u64) {
            Some(chain_id) => chain_id,
            None => {
                let file = chain_id_entry
                    .ok_or("the manifest has neither a chain_id header nor an eth_chainId entry")?;
                let body: Value = serde_json::from_str(
                    &fs::read_to_string(dir.join(&file))
                        .map_err(|err| format!("could not read {file}: {err}"))?,
                )
                .map_err(|err| format!("{file} is not JSON: {err}"))?;
                body.get("result")
                    .and_then(Value::as_str)
                    .and_then(parse_hex_u64)
                    .ok_or_else(|| format!("{file} carries no readable chain id"))?
            }
        };

        Ok(Self {
            dir: dir.to_path_buf(),
            chain_id,
            by_key,
            served: 0,
            missing: Vec::new(),
        })
    }

    /// Number of distinct bodies the bundle holds.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.by_key.len()
    }

    /// Every read that was asked for and not held, in order.
    pub fn missing(&self) -> &[Miss] {
        &self.missing
    }
}

impl ReadSource for BundleSource {
    fn fetch(&mut self, descriptor: Descriptor) -> Result<Fetched, RpcError> {
        let key = cache_key(self.chain_id, &descriptor);
        let Some(held) = self.by_key.get(&key) else {
            self.missing.push(Miss {
                label: descriptor.label.clone(),
                method: descriptor.method.clone(),
                key: key.clone(),
            });
            return Err(RpcError {
                kind: RpcErrorKind::OfflineMiss,
                message: format!(
                    "the bundle holds no body for {} ({}) (key {key})",
                    descriptor.label, descriptor.method
                ),
            });
        };
        let path = self.dir.join(&held.file);
        let body = fs::read_to_string(&path).map_err(|err| RpcError {
            kind: RpcErrorKind::Failed,
            message: format!("could not read {}: {err}", path.display()),
        })?;
        self.served += 1;
        Ok(Fetched {
            descriptor,
            key,
            body,
            cache_hit: true,
            endpoint: held.endpoint.clone(),
            stored_utc: held.first_stored_utc.clone(),
        })
    }

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn counters(&self) -> (usize, usize) {
        (0, self.served)
    }

    fn meta(&self) -> Value {
        json!({
            "source": "bundle",
            "source_bundle": self.dir.file_name().and_then(|n| n.to_str()),
            "endpoints_configured": [],
            "log_endpoints_configured": [],
            "network_calls_this_run": 0,
            "cache_hits_this_run": self.served,
            "rpc_observations": [],
            "endpoint_fingerprints": [],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleWriter;
    use crate::rpc::{call_descriptor, chain_id_descriptor};

    fn fetched(descriptor: Descriptor, body: &str) -> Fetched {
        Fetched {
            key: cache_key(1, &descriptor),
            descriptor,
            body: body.to_string(),
            cache_hit: false,
            endpoint: "https://rpc.example/v1/<redacted>".to_string(),
            stored_utc: "2026-01-01T00:00:00.000Z".to_string(),
        }
    }

    /// Spec 03 R6. The source is built from a bundle written by the writer,
    /// serves the verbatim body for a held key, and records an OfflineMiss
    /// for an unknown one. It owns no HTTP agent: the struct has no such
    /// field, so there is nothing that could open a socket.
    #[test]
    fn bundle_source_serves_bodies_by_key_and_never_opens_a_socket() {
        let root = std::env::temp_dir().join("crossfoot-bundle-source");
        let _ = fs::remove_dir_all(&root);
        let mut writer = BundleWriter::create(&root, "svzchf-run-1-2-stamp", 1).unwrap();
        let chain = fetched(
            chain_id_descriptor(),
            r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#,
        );
        let price = call_descriptor("vault.price()", "0xAbC", "0xa035b1fe", "0x2");
        let price_body = r#"{"jsonrpc":"2.0","id":1,"result":"0x0000000000000000000000000000000000000000000000000e2d2ce73d2a4d70"}"#;
        writer.record(&chain, None, None).unwrap();
        writer
            .record(&fetched(price.clone(), price_body), None, None)
            .unwrap();
        writer.write_manifest("svzchf-run", json!({})).unwrap();

        let mut source = BundleSource::open(writer.dir()).unwrap();
        assert_eq!(source.chain_id(), 1);
        assert_eq!(source.held(), 2);

        // A held key, asked for with a different label: the body comes back
        // byte for byte, because the label is not part of the key.
        let mut relabelled = price.clone();
        relabelled.label = "anything".to_string();
        let served = source.fetch(relabelled).unwrap();
        assert_eq!(served.body, price_body);
        assert!(served.cache_hit);
        assert_eq!(served.endpoint, "https://rpc.example/v1/<redacted>");

        // An unknown key is an OfflineMiss and is remembered.
        let other = call_descriptor("vault.totalSupply()", "0xAbC", "0x18160ddd", "0x2");
        let err = source.fetch(other.clone()).unwrap_err();
        assert!(
            matches!(err.kind, RpcErrorKind::OfflineMiss),
            "{}",
            err.message
        );
        assert_eq!(
            source.missing(),
            &[Miss {
                label: "vault.totalSupply()".to_string(),
                method: "eth_call".to_string(),
                key: cache_key(1, &other),
            }]
        );
        assert_eq!(source.counters(), (0, 1));
        assert_eq!(source.meta()["network_calls_this_run"], 0);
    }
}
