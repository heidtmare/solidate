//! Diagram fences: fenced code blocks whose language is in [`DIAGRAM_LANGUAGES`].
//!
//! A diagram is shared by both variants rather than translated. Sync hashes keep the
//! fence but not its contents (see [`crate::markdown::sync_hash`]), so adding or
//! removing a diagram is a change and editing one is not. [`follow`] carries diagram
//! edits into identical copies in the other variant.

use std::collections::HashSet;
use std::ops::Range;

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};

use crate::markdown::{PREAMBLE_ANCHOR, options, section_starts};

/// Fence languages treated as diagrams.
pub const DIAGRAM_LANGUAGES: &[&str] = &["mermaid"];

/// Whether a fence info string names a diagram language.
pub fn is_diagram(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|lang| DIAGRAM_LANGUAGES.contains(&lang))
}

/// A closed, top-level diagram fence.
struct Fence {
    anchor: String,
    lang: String,
    literal: String,
    /// 0-based line range of the contents, between the opening and closing fence.
    inner: Range<usize>,
}

fn fences(md: &str) -> Vec<Fence> {
    let arena = Arena::new();
    let root = parse_document(&arena, md, &options());
    let starts = section_starts(root);
    root.children()
        .filter_map(|n| {
            let data = n.data();
            let NodeValue::CodeBlock(cb) = &data.value else {
                return None;
            };
            if !cb.fenced || !cb.closed || !is_diagram(&cb.info) {
                return None;
            }
            let (open, close) = (data.sourcepos.start.line, data.sourcepos.end.line);
            let anchor = starts
                .iter()
                .rev()
                .find(|(line, _)| *line <= open)
                .map_or(PREAMBLE_ANCHOR, |(_, a)| a.as_str());
            Some(Fence {
                anchor: anchor.to_owned(),
                lang: cb.info.split_whitespace().next().unwrap_or_default().to_owned(),
                literal: cb.literal.clone(),
                inner: open..close - 1,
            })
        })
        .collect()
}

/// `other` with the diagrams that changed from `old` to `new` updated, or `None` when
/// nothing in `other` changes.
///
/// Diagrams are paired by position within their section, only in sections whose
/// diagram count is the same in `old` and `new`. A changed diagram is carried over
/// when the section with the same anchor in `other` holds exactly one copy of its old
/// contents. Added, removed and diverged diagrams are left alone.
pub fn follow(old: &str, new: &str, other: &str) -> Option<String> {
    let (old_f, new_f, other_f) = (fences(old), fences(new), fences(other));
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let in_section = |f: &[Fence], anchor: &str| f.iter().filter(|x| x.anchor == anchor).count();

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut targets = HashSet::new();
    let mut seen = HashSet::new();
    for anchor in new_f.iter().map(|f| f.anchor.as_str()) {
        if !seen.insert(anchor) || in_section(&old_f, anchor) != in_section(&new_f, anchor) {
            continue;
        }
        let pairs = old_f
            .iter()
            .filter(|f| f.anchor == anchor)
            .zip(new_f.iter().filter(|f| f.anchor == anchor));
        for (was, now) in pairs {
            if was.lang != now.lang || was.literal == now.literal {
                continue;
            }
            let mut copies = other_f
                .iter()
                .filter(|c| c.anchor == anchor && c.lang == was.lang && c.literal == was.literal);
            if let (Some(copy), None) = (copies.next(), copies.next())
                && targets.insert(copy.inner.start)
            {
                edits.push((copy.inner.clone(), new_lines[now.inner.clone()].concat()));
            }
        }
    }
    if edits.is_empty() {
        return None;
    }

    edits.sort_by_key(|(r, _)| r.start);
    let lines: Vec<&str> = other.split_inclusive('\n').collect();
    let mut out = String::with_capacity(other.len());
    let mut at = 0;
    for (range, text) in edits {
        out.push_str(&lines[at..range.start].concat());
        out.push_str(&text);
        at = range.end;
    }
    out.push_str(&lines[at..].concat());
    (out != other).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "# Flow\n\nText.\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n# Other\n\n```mermaid\nX\n```\n";
    const NEW: &str =
        "# Flow\n\nText, reworded.\n\n```mermaid\ngraph TD\n  A-->C\n```\n\n# Other\n\n```mermaid\nX\n```\n";

    #[test]
    fn detects_diagram_languages() {
        assert!(is_diagram("mermaid"));
        assert!(is_diagram("mermaid title=x"));
        assert!(!is_diagram("rust"));
        assert!(!is_diagram(""));
    }

    #[test]
    fn carries_edits_into_identical_copies() {
        let other =
            "# Flow\n\n- dense: a to b\n\n~~~~ mermaid\ngraph TD\n  A-->B\n~~~~\n\n# Other\n\n```mermaid\nX\n```\n";
        assert_eq!(
            follow(OLD, NEW, other).as_deref(),
            Some(
                "# Flow\n\n- dense: a to b\n\n~~~~ mermaid\ngraph TD\n  A-->C\n~~~~\n\n# Other\n\n```mermaid\nX\n```\n"
            )
        );
    }

    #[test]
    fn leaves_diverged_missing_and_misplaced_copies() {
        let diverged = "# Flow\n\n```mermaid\ngraph TD\n  A-->Z\n```\n";
        assert_eq!(follow(OLD, NEW, diverged), None);
        assert_eq!(follow(OLD, NEW, "# Flow\n\nNo diagram.\n"), None);
        let elsewhere = "# Other\n\n```mermaid\ngraph TD\n  A-->B\n```\n";
        assert_eq!(follow(OLD, NEW, elsewhere), None);
        let twice = "# Flow\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n```mermaid\ngraph TD\n  A-->B\n```\n";
        assert_eq!(follow(OLD, NEW, twice), None, "ambiguous copy");
    }

    #[test]
    fn ignores_added_and_removed_diagrams() {
        let added = "# Flow\n\n```mermaid\nnew\n```\n\n```mermaid\ngraph TD\n  A-->C\n```\n";
        let other = "# Flow\n\n```mermaid\ngraph TD\n  A-->B\n```\n";
        assert_eq!(follow(OLD, added, other), None);
        assert_eq!(follow(OLD, "# Flow\n\nGone.\n", other), None);
    }

    #[test]
    fn ignores_unclosed_and_nested_fences() {
        let old = "# F\n\n> ```mermaid\n> A\n> ```\n";
        let new = "# F\n\n> ```mermaid\n> B\n> ```\n";
        assert_eq!(follow(old, new, old), None);
        assert_eq!(
            follow(
                "# F\n\n```mermaid\nA\n",
                "# F\n\n```mermaid\nB\n",
                "# F\n\n```mermaid\nA\n"
            ),
            None
        );
    }
}
