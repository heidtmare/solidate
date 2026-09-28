//! Source bindings: which repository files a document section describes, and
//! whether those files changed since the section was last checked against them.
//!
//! A section declares its sources with an HTML comment on its own lines (an HTML
//! block at the top level of the document):
//!
//! ```md
//! ## Token validation {#tokens}
//!
//! <!-- sources: crates/solidate-app/src/auth.rs, crates/solidate-db/migrations/ -->
//! ```
//!
//! Patterns are separated by commas or newlines and are relative to the repository
//! root. `*` and `?` match within one path segment, `**` matches any number of
//! segments, and a trailing `/` means everything below a directory. A directive
//! belongs to the top-level section it appears in; before the first heading it
//! belongs to the preamble anchor. Bindings of a document are the union of both
//! variants' directives, keyed by anchor. Directives are excluded from semantic
//! hashes, so adding or editing one never marks the other variant stale.
//!
//! Clients (CI, agents) report the repository's files as `path → hash`. Hashes are
//! opaque to Solidate; clients report git blob object ids so that every reporter
//! agrees. Verifying a section records the hashes of the files its patterns match.
//! Comparing that record with the current report gives the section's [`DriftState`].

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Reported repository files: path → content hash.
pub type Files = BTreeMap<String, String>;

pub const MAX_PATH_LEN: usize = 1024;
pub const MAX_HASH_LEN: usize = 128;

/// The source patterns one section declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBinding {
    pub anchor: String,
    /// Raw patterns in declaration order, de-duplicated.
    pub patterns: Vec<String>,
}

/// Patterns of a `<!-- sources: ... -->` HTML block, or `None` if `html` is not one.
pub fn parse_directive(html: &str) -> Option<Vec<String>> {
    let inner = html.trim().strip_prefix("<!--")?.strip_suffix("-->")?;
    let list = inner.trim_start().strip_prefix("sources:")?;
    Some(
        list.split([',', '\n'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// Merges bindings by anchor, keeping first-seen order of anchors and patterns.
pub fn merge_bindings(bindings: impl IntoIterator<Item = SourceBinding>) -> Vec<SourceBinding> {
    let mut out: Vec<SourceBinding> = Vec::new();
    for b in bindings {
        let slot = match out.iter_mut().position(|x| x.anchor == b.anchor) {
            Some(i) => &mut out[i],
            None => {
                out.push(SourceBinding {
                    anchor: b.anchor,
                    patterns: Vec::new(),
                });
                out.last_mut().expect("just pushed")
            }
        };
        for p in b.patterns {
            if !slot.patterns.contains(&p) {
                slot.patterns.push(p);
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid source {kind} {value:?}: {reason}")]
pub struct SourceError {
    kind: &'static str,
    value: String,
    reason: &'static str,
}

fn err(kind: &'static str, value: &str, reason: &'static str) -> SourceError {
    SourceError {
        kind,
        value: value.to_owned(),
        reason,
    }
}

/// Splits a relative path into segments, rejecting absolute paths, empty, `.` and
/// `..` segments, backslashes and control characters.
fn segments<'a>(kind: &'static str, s: &'a str) -> Result<Vec<&'a str>, SourceError> {
    if s.is_empty() {
        return Err(err(kind, s, "empty"));
    }
    if s.len() > MAX_PATH_LEN {
        return Err(err(kind, s, "too long"));
    }
    if s.starts_with('/') {
        return Err(err(kind, s, "must be relative to the repository root"));
    }
    if s.contains('\\') || s.chars().any(char::is_control) {
        return Err(err(kind, s, "backslashes and control characters are not allowed"));
    }
    let segs: Vec<&str> = s.split('/').collect();
    if segs.iter().any(|g| *g == "." || *g == "..") {
        return Err(err(kind, s, "`.` and `..` segments are not allowed"));
    }
    Ok(segs)
}

/// Checks a reported file path and hash.
pub fn validate_file(path: &str, hash: &str) -> Result<(), SourceError> {
    if segments("path", path)?.iter().any(|g| g.is_empty()) {
        return Err(err("path", path, "empty segment"));
    }
    if hash.is_empty() || hash.len() > MAX_HASH_LEN || !hash.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(err("hash", hash, "expected 1 to 128 printable ASCII characters"));
    }
    Ok(())
}

/// A validated source pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePattern {
    raw: String,
    segs: Vec<String>,
}

impl SourcePattern {
    pub fn parse(raw: &str) -> Result<Self, SourceError> {
        let mut segs: Vec<String> = segments("pattern", raw)?.into_iter().map(str::to_owned).collect();
        if segs.last().is_some_and(String::is_empty) {
            *segs.last_mut().expect("non-empty") = "**".to_owned();
        }
        if segs.iter().any(String::is_empty) {
            return Err(err("pattern", raw, "empty segment"));
        }
        Ok(Self {
            raw: raw.to_owned(),
            segs,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether the pattern names exactly one path.
    fn literal(&self) -> Option<String> {
        (!self.segs.iter().any(|s| s.contains(['*', '?']))).then(|| self.segs.join("/"))
    }

    pub fn matches(&self, path: &str) -> bool {
        let path: Vec<&str> = path.split('/').collect();
        match_segments(&self.segs, &path)
    }
}

impl fmt::Display for SourcePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn match_segments(pat: &[String], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((p, rest)) if p == "**" => (0..=path.len()).any(|i| match_segments(rest, &path[i..])),
        Some((p, rest)) => path
            .split_first()
            .is_some_and(|(s, tail)| wildcard(p, s) && match_segments(rest, tail)),
    }
}

/// `*` matches any run of characters, `?` exactly one.
fn wildcard(pat: &str, s: &str) -> bool {
    let (p, s): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, si));
                pi += 1;
            }
            Some(&c) if c == '?' || c == s[si] => {
                pi += 1;
                si += 1;
            }
            _ => match star {
                Some((sp, ss)) => {
                    pi = sp + 1;
                    si = ss + 1;
                    star = Some((sp, ss + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Parses `raw` patterns, silently dropping invalid ones (they match nothing).
pub fn parse_patterns(raw: &[String]) -> Vec<SourcePattern> {
    raw.iter().filter_map(|p| SourcePattern::parse(p).ok()).collect()
}

/// Files matched by any of `patterns`.
pub fn matched(patterns: &[SourcePattern], files: &Files) -> Files {
    let mut out = Files::new();
    for p in patterns {
        match p.literal() {
            Some(path) => {
                if let Some((k, v)) = files.get_key_value(&path) {
                    out.insert(k.clone(), v.clone());
                }
            }
            None => out.extend(
                files
                    .iter()
                    .filter(|(k, _)| p.matches(k))
                    .map(|(k, v)| (k.clone(), v.clone())),
            ),
        }
    }
    out
}

/// Raw patterns among `raw` that match no file in `files` (including invalid ones).
pub fn missing(raw: &[String], files: &Files) -> Vec<String> {
    raw.iter()
        .filter(|r| match SourcePattern::parse(r) {
            Ok(p) => matched(std::slice::from_ref(&p), files).is_empty(),
            Err(_) => true,
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftState {
    /// The section has never been verified against its sources.
    Unverified,
    /// Matched files changed, appeared or disappeared since the last verification.
    Changed,
    /// Matched files are exactly those recorded at the last verification.
    Fresh,
}

impl DriftState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Changed => "changed",
            Self::Fresh => "fresh",
        }
    }
}

impl fmt::Display for DriftState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Differences between the files recorded at verification and those matched now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    /// Present in both with a different hash.
    pub changed: Vec<String>,
    /// Matched now, not recorded.
    pub added: Vec<String>,
    /// Recorded, not matched now.
    pub removed: Vec<String>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

pub fn delta(recorded: &Files, current: &Files) -> Delta {
    let mut d = Delta::default();
    for (path, hash) in current {
        match recorded.get(path) {
            Some(h) if h == hash => {}
            Some(_) => d.changed.push(path.clone()),
            None => d.added.push(path.clone()),
        }
    }
    d.removed = recorded.keys().filter(|p| !current.contains_key(*p)).cloned().collect();
    d
}

/// Drift state of a section from its verification record (`None`: never verified)
/// and the files its patterns match now.
pub fn classify(recorded: Option<&Files>, current: &Files) -> (DriftState, Delta) {
    match recorded {
        None => (DriftState::Unverified, Delta::default()),
        Some(r) => {
            let d = delta(r, current);
            let state = if d.is_empty() {
                DriftState::Fresh
            } else {
                DriftState::Changed
            };
            (state, d)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> Files {
        entries.iter().map(|(p, h)| (p.to_string(), h.to_string())).collect()
    }

    fn m(p: &str, path: &str) -> bool {
        SourcePattern::parse(p).unwrap().matches(path)
    }

    #[test]
    fn directive_parsing() {
        assert_eq!(
            parse_directive("<!-- sources: a.rs, src/*.rs\n  docs/ -->\n"),
            Some(vec!["a.rs".into(), "src/*.rs".into(), "docs/".into()])
        );
        assert_eq!(parse_directive("<!--sources:-->"), Some(vec![]));
        assert_eq!(parse_directive("<!-- note: x -->"), None);
        assert_eq!(parse_directive("<div>sources: x</div>"), None);
    }

    #[test]
    fn glob_matching() {
        assert!(m("src/lib.rs", "src/lib.rs"));
        assert!(!m("src/lib.rs", "src/lib.rs.bak"));
        assert!(m("src/*.rs", "src/lib.rs"));
        assert!(!m("src/*.rs", "src/a/lib.rs"));
        assert!(m("src/**/*.rs", "src/lib.rs"));
        assert!(m("src/**/*.rs", "src/a/b/lib.rs"));
        assert!(m("src/", "src/a/b/lib.rs"));
        assert!(!m("src/", "srcx/lib.rs"));
        assert!(m("**/Cargo.toml", "Cargo.toml"));
        assert!(m("**/Cargo.toml", "crates/x/Cargo.toml"));
        assert!(m("migrations/000?_*.sql", "migrations/0003_proposals.sql"));
        assert!(!m("migrations/000?_*.sql", "migrations/0010_x.sql"));
        assert!(m("*", "README.md") && !m("*", "a/README.md"));
    }

    #[test]
    fn rejects_bad_patterns_and_files() {
        for p in ["", "/abs", "a/../b", "./a", "a//b", "a\\b"] {
            assert!(SourcePattern::parse(p).is_err(), "{p:?}");
        }
        assert!(validate_file("src/a.rs", "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").is_ok());
        assert!(validate_file("src/", "h").is_err());
        assert!(validate_file("a", "").is_err());
        assert!(validate_file("a", "has space").is_err());
    }

    #[test]
    fn matching_and_missing_patterns() {
        let f = files(&[("src/a.rs", "1"), ("src/b.rs", "2"), ("docs/x.md", "3")]);
        let raw = vec!["src/*.rs".to_owned(), "gone.rs".to_owned(), "/bad".to_owned()];
        let got = matched(&parse_patterns(&raw), &f);
        assert_eq!(got, files(&[("src/a.rs", "1"), ("src/b.rs", "2")]));
        assert_eq!(missing(&raw, &f), ["gone.rs", "/bad"]);
    }

    #[test]
    fn classification() {
        let rec = files(&[("a", "1"), ("b", "2"), ("c", "3")]);
        assert_eq!(classify(None, &rec).0, DriftState::Unverified);
        assert_eq!(classify(Some(&rec), &rec), (DriftState::Fresh, Delta::default()));
        let now = files(&[("a", "1"), ("b", "9"), ("d", "4")]);
        let (state, d) = classify(Some(&rec), &now);
        assert_eq!(state, DriftState::Changed);
        assert_eq!(
            d,
            Delta {
                changed: vec!["b".into()],
                added: vec!["d".into()],
                removed: vec!["c".into()],
            }
        );
    }

    #[test]
    fn merging_bindings() {
        let b = |a: &str, p: &[&str]| SourceBinding {
            anchor: a.into(),
            patterns: p.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(
            merge_bindings([b("x", &["1", "2"]), b("y", &["3"]), b("x", &["2", "4"])]),
            [b("x", &["1", "2", "4"]), b("y", &["3"])]
        );
    }
}
