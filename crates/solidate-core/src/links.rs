//! Link and include targets.
//!
//! Solidate recognizes three reference forms:
//!
//! - Markdown links, `[text](auth#tokens)`. The target resolves relative to the
//!   current document, or from the project root when it starts with `/`. A URL with
//!   a scheme (`https:`, `mailto:`) is external and isn't tracked.
//! - Wiki links, `[[design/auth#tokens]]` or `[[shared:rules/style]]`. These are always
//!   project-rooted and may name another project with a `project:` prefix.
//! - Includes, `{{include shared:rules/style#tone}}`, written alone on a line. They
//!   use the same target syntax as wiki links.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::path::{DocPath, Slug};

/// Where a link points. When `path` is `None`, it points within the current document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinkTarget {
    pub project: Option<Slug>,
    pub path: Option<DocPath>,
    pub anchor: Option<String>,
}

impl fmt::Display for LinkTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(p) = &self.project {
            write!(f, "{p}:")?;
        }
        if let Some(p) = &self.path {
            write!(f, "{p}")?;
        }
        if let Some(a) = &self.anchor {
            write!(f, "#{a}")?;
        }
        Ok(())
    }
}

fn split_anchor(s: &str) -> (&str, Option<String>) {
    match s.split_once('#') {
        Some((p, a)) if !a.is_empty() => (p, Some(a.to_owned())),
        Some((p, _)) => (p, None),
        None => (s, None),
    }
}

/// Parses a wiki-link or include target: `[project:]path[#anchor]`.
pub fn parse_wiki_target(raw: &str) -> Option<LinkTarget> {
    let raw = raw.trim();
    let (project, rest) = match raw.split_once(':') {
        Some((p, rest)) => (Some(Slug::parse(p).ok()?), rest),
        None => (None, raw),
    };
    let (path, anchor) = split_anchor(rest);
    let path = if path.is_empty() {
        None
    } else {
        Some(DocPath::parse(path).ok()?)
    };
    if path.is_none() && project.is_some() {
        return None;
    }
    if path.is_none() && anchor.is_none() {
        return None;
    }
    Some(LinkTarget { project, path, anchor })
}

/// Parses a Markdown link URL. Returns `None` for external or unparseable URLs.
/// `base` is the linking document, used to resolve relative paths.
pub fn parse_markdown_link(url: &str, base: Option<&DocPath>) -> Option<LinkTarget> {
    let url = url.trim();
    if url.is_empty() || url.starts_with("//") || has_scheme(url) {
        return None;
    }
    let url = url.split('?').next().unwrap_or(url);
    let (path, anchor) = split_anchor(url);
    let path = match (path.is_empty(), base) {
        (true, _) => None,
        (false, Some(base)) => Some(base.join(path).ok()?),
        (false, None) => Some(DocPath::parse(path).ok()?),
    };
    if path.is_none() && anchor.is_none() {
        return None;
    }
    Some(LinkTarget {
        project: None,
        path,
        anchor,
    })
}

fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    !scheme.contains('/')
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Parses a `{{include target}}` directive. Returns `None` if `line` isn't one.
pub fn parse_include(line: &str) -> Option<LinkTarget> {
    let inner = line.trim().strip_prefix("{{")?.strip_suffix("}}")?.trim();
    let target = inner.strip_prefix("include")?;
    if !target.starts_with(char::is_whitespace) {
        return None;
    }
    // Comrak's CommonMark writer escapes `#` in text; accept either form.
    parse_wiki_target(&target.replace("\\#", "#")).filter(|t| t.path.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(project: Option<&str>, path: Option<&str>, anchor: Option<&str>) -> LinkTarget {
        LinkTarget {
            project: project.map(|p| Slug::parse(p).unwrap()),
            path: path.map(|p| DocPath::parse(p).unwrap()),
            anchor: anchor.map(str::to_owned),
        }
    }

    #[test]
    fn wiki_targets() {
        assert_eq!(
            parse_wiki_target("design/auth"),
            Some(t(None, Some("design/auth"), None))
        );
        assert_eq!(
            parse_wiki_target("shared:rules/style#tone"),
            Some(t(Some("shared"), Some("rules/style"), Some("tone")))
        );
        assert_eq!(parse_wiki_target("#local"), Some(t(None, None, Some("local"))));
        assert_eq!(parse_wiki_target("Bad Slug:x"), None);
        assert_eq!(parse_wiki_target("shared:"), None);
    }

    #[test]
    fn markdown_links() {
        let base = DocPath::parse("design/auth").unwrap();
        assert_eq!(parse_markdown_link("https://x.dev/a", Some(&base)), None);
        assert_eq!(parse_markdown_link("mailto:a@b", Some(&base)), None);
        assert_eq!(
            parse_markdown_link("sessions.md#ttl", Some(&base)),
            Some(t(None, Some("design/sessions"), Some("ttl")))
        );
        assert_eq!(
            parse_markdown_link("/readme", Some(&base)),
            Some(t(None, Some("readme"), None))
        );
        assert_eq!(
            parse_markdown_link("#top", Some(&base)),
            Some(t(None, None, Some("top")))
        );
    }

    #[test]
    fn includes() {
        assert_eq!(
            parse_include("  {{include shared:rules#x}} "),
            Some(t(Some("shared"), Some("rules"), Some("x")))
        );
        assert_eq!(
            parse_include("{{include shared:rules\\#x}}").unwrap().anchor.as_deref(),
            Some("x")
        );
        assert_eq!(parse_include("{{included x}}"), None);
        assert_eq!(parse_include("{{include #only-anchor}}"), None);
        assert_eq!(parse_include("text {{include x}}"), None);
    }
}
