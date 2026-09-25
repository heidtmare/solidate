//! Validated identifiers: document paths and slugs.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path segment {0:?} is not allowed")]
    BadSegment(String),
    #[error("path is longer than {max} characters")]
    TooLong { max: usize },
}

/// A project-relative document path such as `design/auth`.
///
/// Segments contain ASCII letters, digits, `-`, `_` and `.`, and may not be `.` or
/// `..`. Leading and trailing slashes are dropped, and a trailing `.md` is removed, so
/// `/design/auth.md` and `design/auth` name the same document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DocPath(String);

impl DocPath {
    pub const MAX_LEN: usize = 512;

    pub fn parse(raw: &str) -> Result<Self, PathError> {
        let trimmed = raw.trim().trim_matches('/');
        let trimmed = trimmed.strip_suffix(".md").unwrap_or(trimmed);
        if trimmed.is_empty() {
            return Err(PathError::Empty);
        }
        if trimmed.len() > Self::MAX_LEN {
            return Err(PathError::TooLong { max: Self::MAX_LEN });
        }
        for seg in trimmed.split('/') {
            if !is_valid_segment(seg) {
                return Err(PathError::BadSegment(seg.to_owned()));
            }
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The last segment, e.g. `auth` for `design/auth`.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The directory part, e.g. `design` for `design/auth`, or `""` at the root.
    pub fn dir(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// Resolves `target` against this document's directory. `/x` is project-rooted,
    /// and `..` segments are allowed as long as they stay inside the project.
    pub fn join(&self, target: &str) -> Result<Self, PathError> {
        let target = target.trim();
        let mut segs: Vec<&str> = if target.starts_with('/') {
            Vec::new()
        } else {
            self.dir().split('/').filter(|s| !s.is_empty()).collect()
        };
        for seg in target.split('/').filter(|s| !s.is_empty()) {
            match seg {
                "." => {}
                ".." => {
                    if segs.pop().is_none() {
                        return Err(PathError::BadSegment("..".into()));
                    }
                }
                s => segs.push(s),
            }
        }
        Self::parse(&segs.join("/"))
    }
}

fn is_valid_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl fmt::Display for DocPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DocPath {
    type Err = PathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for DocPath {
    type Error = PathError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<DocPath> for String {
    fn from(p: DocPath) -> Self {
        p.0
    }
}

/// A URL-safe identifier for tenants and projects: lowercase ASCII letters, digits and
/// `-`. It must start with a letter and be 1–63 characters long.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Slug(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid slug {0:?}: use 1-63 lowercase letters, digits or '-', starting with a letter")]
pub struct SlugError(pub String);

impl Slug {
    pub fn parse(raw: &str) -> Result<Self, SlugError> {
        let ok = (1..=63).contains(&raw.len())
            && raw.as_bytes()[0].is_ascii_lowercase()
            && raw
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(raw.to_owned()))
        } else {
            Err(SlugError(raw.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Slug {
    type Err = SlugError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for Slug {
    type Error = SlugError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<Slug> for String {
    fn from(s: Slug) -> Self {
        s.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_path_normalizes() {
        assert_eq!(DocPath::parse("/design/auth.md").unwrap().as_str(), "design/auth");
        assert_eq!(DocPath::parse("readme").unwrap().dir(), "");
        let p = DocPath::parse("a/b/c").unwrap();
        assert_eq!((p.dir(), p.name()), ("a/b", "c"));
    }

    #[test]
    fn doc_path_rejects_bad_input() {
        assert_eq!(DocPath::parse(""), Err(PathError::Empty));
        assert!(DocPath::parse("a//b").is_err());
        assert!(DocPath::parse("a/../b").is_err());
        assert!(DocPath::parse("a b").is_err());
    }

    #[test]
    fn join_resolves_relative_and_rooted() {
        let p = DocPath::parse("design/auth").unwrap();
        assert_eq!(p.join("sessions").unwrap().as_str(), "design/sessions");
        assert_eq!(p.join("../ops/deploy.md").unwrap().as_str(), "ops/deploy");
        assert_eq!(p.join("/readme").unwrap().as_str(), "readme");
        assert!(p.join("../../x").is_err());
    }

    #[test]
    fn slug_rules() {
        assert!(Slug::parse("acme-2").is_ok());
        for bad in ["", "Acme", "2acme", "a_b", &"a".repeat(64)] {
            assert!(Slug::parse(bad).is_err(), "{bad:?}");
        }
    }
}
