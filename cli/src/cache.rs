//! Content addressed cache for JSON-RPC responses.
//!
//! Key: sha256 over the canonical preimage of (chain_id, method, block, to,
//! calldata). The endpoint is deliberately NOT part of the key: a read pinned
//! to a block number is a property of the chain, not of the node that served
//! it, so two endpoints must be interchangeable for the same key. Which
//! endpoint actually served a body is recorded in the sidecar metadata and in
//! the bundle manifest.
//!
//! Value: the response body verbatim, byte for byte as the node sent it, in
//! `<key>.body`. A sidecar `<key>.meta.json` carries the descriptor, the
//! endpoint and the store timestamp. Keeping them in separate files is what
//! lets the body stay verbatim.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rpc::Descriptor;

pub const PREIMAGE_VERSION: &str = "crossfoot-cache-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMeta {
    pub key: String,
    pub chain_id: u64,
    pub method: String,
    pub block: String,
    pub to: String,
    pub calldata: String,
    pub request: serde_json::Value,
    pub endpoint: String,
    pub stored_utc: String,
}

/// The exact byte string that is hashed to form the cache key. Written out in
/// full so a third party can recompute a key without reading this code.
pub fn key_preimage(chain_id: u64, descriptor: &Descriptor) -> String {
    format!(
        "{PREIMAGE_VERSION}\nchain_id={}\nmethod={}\nblock={}\nto={}\ncalldata={}\n",
        chain_id,
        descriptor.method,
        descriptor.block.to_lowercase(),
        descriptor.to.to_lowercase(),
        descriptor.calldata.to_lowercase(),
    )
}

pub fn cache_key(chain_id: u64, descriptor: &Descriptor) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key_preimage(chain_id, descriptor).as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir_for(&self, key: &str) -> PathBuf {
        self.root.join(&key[0..2])
    }

    fn body_path(&self, key: &str) -> PathBuf {
        self.dir_for(key).join(format!("{key}.body"))
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.dir_for(key).join(format!("{key}.meta.json"))
    }

    /// Returns the verbatim body and its metadata, or None on a miss. A body
    /// without readable metadata is treated as a miss rather than repaired.
    pub fn get(&self, key: &str) -> Option<(String, CacheMeta)> {
        let body = fs::read_to_string(self.body_path(key)).ok()?;
        let meta_raw = fs::read_to_string(self.meta_path(key)).ok()?;
        let meta: CacheMeta = serde_json::from_str(&meta_raw).ok()?;
        Some((body, meta))
    }

    pub fn put(&self, key: &str, body: &str, meta: &CacheMeta) -> io::Result<()> {
        let dir = self.dir_for(key);
        fs::create_dir_all(&dir)?;
        fs::write(self.body_path(key), body.as_bytes())?;
        let meta_json = serde_json::to_string_pretty(meta)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        fs::write(self.meta_path(key), meta_json.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn descriptor() -> Descriptor {
        Descriptor {
            label: "vault.convertToAssets(1e18)".to_string(),
            wire: crate::rpc::Wire::JsonRpc,
            method: "eth_call".to_string(),
            block: "0x176E890".to_string(),
            to: "0xE5F130253fF137f9917C0107659A4c5262abf6b0".to_string(),
            calldata: "0x07A2D13A".to_string(),
            params: json!([]),
        }
    }

    #[test]
    fn key_is_case_insensitive_for_hex_fields() {
        let mut lowered = descriptor();
        lowered.block = lowered.block.to_lowercase();
        lowered.to = lowered.to.to_lowercase();
        lowered.calldata = lowered.calldata.to_lowercase();
        assert_eq!(cache_key(1, &descriptor()), cache_key(1, &lowered));
    }

    #[test]
    fn key_changes_with_every_component() {
        let base = cache_key(1, &descriptor());
        assert_ne!(base, cache_key(8453, &descriptor()));

        let mut other_block = descriptor();
        other_block.block = "0x176e891".to_string();
        assert_ne!(base, cache_key(1, &other_block));

        let mut other_to = descriptor();
        other_to.to = "0x27d9AD987BdE08a0d083ef7e0e4043C857A17B38".to_string();
        assert_ne!(base, cache_key(1, &other_to));

        let mut other_calldata = descriptor();
        other_calldata.calldata = "0x18160ddd".to_string();
        assert_ne!(base, cache_key(1, &other_calldata));

        let mut other_method = descriptor();
        other_method.method = "eth_getCode".to_string();
        assert_ne!(base, cache_key(1, &other_method));
    }

    /// The label is documentation, not identity: renaming a read must not
    /// invalidate an already cached response.
    #[test]
    fn key_ignores_the_label() {
        let mut relabelled = descriptor();
        relabelled.label = "something else entirely".to_string();
        assert_eq!(cache_key(1, &descriptor()), cache_key(1, &relabelled));
    }

    #[test]
    fn preimage_is_the_documented_shape() {
        assert_eq!(
            key_preimage(1, &descriptor()),
            "crossfoot-cache-v1\nchain_id=1\nmethod=eth_call\nblock=0x176e890\nto=0xe5f130253ff137f9917c0107659a4c5262abf6b0\ncalldata=0x07a2d13a\n"
        );
    }
}
