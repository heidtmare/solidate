//! Content hashing.
//!
//! Solidate uses two kinds of hashes, both BLAKE3:
//!
//! - A **content hash** covers the exact bytes a person or tool wrote. It identifies a
//!   blob and is the HTTP `ETag`, so any byte change, even whitespace, produces a new one.
//! - A **semantic hash** covers normalized Markdown (see [`crate::markdown::normalize`]).
//!   Sync tracking uses it so that reformatting a section does not mark its
//!   counterpart stale.
//!
//! [`Merkle`] rolls many hashes into one so a client can check a whole document or
//! project with a single comparison.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A 256-bit BLAKE3 digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash([u8; 32]);

impl Hash {
    pub fn of(bytes: impl AsRef<[u8]>) -> Self {
        Self(*blake3::hash(bytes.as_ref()).as_bytes())
    }

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    /// The first 12 hex characters. Use only for display, never for comparison.
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_owned()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.short())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid hash: expected 64 hex characters")]
pub struct ParseHashError;

impl FromStr for Hash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        blake3::Hash::from_hex(s)
            .map(|h| Self(*h.as_bytes()))
            .map_err(|_| ParseHashError)
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Builds a single root hash from keyed hashes. The result doesn't depend on insertion order.
///
/// Each entry is encoded as `key \0 hex \n` in key order. Keys must not contain
/// `\0` or `\n`; document paths and anchors never do.
#[derive(Debug, Default, Clone)]
pub struct Merkle {
    entries: BTreeMap<String, Hash>,
}

impl Merkle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, hash: Hash) -> &mut Self {
        self.entries.insert(key.into(), hash);
        self
    }

    pub fn finish(&self) -> Hash {
        let mut hasher = blake3::Hasher::new();
        for (key, hash) in &self.entries {
            hasher.update(key.as_bytes());
            hasher.update(b"\0");
            hasher.update(hash.to_hex().as_bytes());
            hasher.update(b"\n");
        }
        Hash(*hasher.finalize().as_bytes())
    }
}

impl<K: Into<String>> FromIterator<(K, Hash)> for Merkle {
    fn from_iter<I: IntoIterator<Item = (K, Hash)>>(iter: I) -> Self {
        let mut m = Self::new();
        for (k, h) in iter {
            m.insert(k, h);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let h = Hash::of("hello");
        assert_eq!(h.to_hex().parse::<Hash>().unwrap(), h);
        assert_eq!(h.short().len(), 12);
        assert!("nope".parse::<Hash>().is_err());
    }

    #[test]
    fn serde_as_hex_string() {
        let h = Hash::of("x");
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, format!("\"{h}\""));
        assert_eq!(serde_json::from_str::<Hash>(&json).unwrap(), h);
    }

    #[test]
    fn merkle_is_order_independent_and_sensitive_to_content() {
        let a: Merkle = [("a", Hash::of("1")), ("b", Hash::of("2"))].into_iter().collect();
        let b: Merkle = [("b", Hash::of("2")), ("a", Hash::of("1"))].into_iter().collect();
        let c: Merkle = [("a", Hash::of("1")), ("b", Hash::of("3"))].into_iter().collect();
        assert_eq!(a.finish(), b.finish());
        assert_ne!(a.finish(), c.finish());
    }
}
