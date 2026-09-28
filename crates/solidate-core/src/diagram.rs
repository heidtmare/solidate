//! Diagram fences: fenced code blocks whose language is in [`DIAGRAM_LANGUAGES`].
//!
//! A diagram is shared by both variants rather than translated. Sync hashes keep the
//! fence but not its contents (see [`crate::markdown::sync_hash`]), so adding or
//! removing a diagram is a change and editing one is not. [`follow`] carries diagram
//! edits into identical copies in the other variant; [`drift`] lists diagrams whose
//! contents differ between variants, and [`copy`] makes one side match the other.

use std::collections::HashSet;
use std::ops::Range;

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};
use serde::Serialize;

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

/// Result of [`follow`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Follow {
    /// `other` with the changed diagrams updated, or `None` when nothing in it changes.
    pub content: Option<String>,
    /// Anchors of sections whose diagram edits were carried into `other`.
    pub carried: Vec<String>,
    /// Anchors of sections with an edited diagram that `other` holds diagrams for but
    /// no single identical copy of, so the edit was not carried over.
    pub skipped: Vec<String>,
}

/// Carries the diagrams that changed from `old` to `new` into `other`.
///
/// Diagrams are paired by position within their section, only in sections whose
/// diagram count is the same in `old` and `new`. A changed diagram is carried over
/// when the section with the same anchor in `other` holds exactly one copy of its old
/// contents. Added, removed and diverged diagrams are left alone.
pub fn follow(old: &str, new: &str, other: &str) -> Follow {
    let (old_f, new_f, other_f) = (fences(old), fences(new), fences(other));
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut targets = HashSet::new();
    let (mut carried, mut skipped) = (Vec::new(), Vec::new());
    for anchor in anchors(&new_f) {
        let (was_s, now_s) = (in_section(&old_f, anchor), in_section(&new_f, anchor));
        if was_s.len() != now_s.len() {
            continue;
        }
        let there = in_section(&other_f, anchor);
        let (mut any_carried, mut any_skipped) = (false, false);
        for (was, now) in was_s.iter().zip(&now_s) {
            if was.lang != now.lang || was.literal == now.literal {
                continue;
            }
            let mut copies = there.iter().filter(|c| c.lang == was.lang && c.literal == was.literal);
            match (copies.next(), copies.next()) {
                (Some(copy), None) if targets.insert(copy.inner.start) => {
                    edits.push((copy.inner.clone(), new_lines[now.inner.clone()].concat()));
                    any_carried = true;
                }
                _ if !there.is_empty() => any_skipped = true,
                _ => {}
            }
        }
        if any_carried {
            carried.push(anchor.to_owned());
        }
        if any_skipped {
            skipped.push(anchor.to_owned());
        }
    }
    let content = (!edits.is_empty()).then(|| splice(other, edits)).filter(|c| c != other);
    Follow {
        content,
        carried,
        skipped,
    }
}

/// A diagram whose contents differ between the human and AI variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drift {
    pub anchor: String,
    /// Position of the diagram among the section's diagrams, from 0.
    pub index: usize,
    pub lang: String,
    pub human: String,
    pub ai: String,
}

/// Diagrams that differ between `human` and `ai`, in `human` document order.
///
/// Diagrams are paired by position within sections of the same anchor, only where
/// both sections hold the same number of diagrams. Sections whose diagram counts
/// differ are sync changes, not drift.
pub fn drift(human: &str, ai: &str) -> Vec<Drift> {
    let (h_f, a_f) = (fences(human), fences(ai));
    let mut out = Vec::new();
    for anchor in anchors(&h_f) {
        let (h, a) = (in_section(&h_f, anchor), in_section(&a_f, anchor));
        if h.len() != a.len() {
            continue;
        }
        for (index, (h, a)) in h.iter().zip(&a).enumerate() {
            if h.lang == a.lang && h.literal != a.literal {
                out.push(Drift {
                    anchor: anchor.to_owned(),
                    index,
                    lang: h.lang.clone(),
                    human: h.literal.clone(),
                    ai: a.literal.clone(),
                });
            }
        }
    }
    out
}

/// `to` with diagram `index` of section `anchor` replaced by the contents of the
/// paired diagram in `from`, or `None` when the diagrams do not pair (see [`drift`])
/// or already match. The fence lines of `to` are kept.
pub fn copy(from: &str, to: &str, anchor: &str, index: usize) -> Option<String> {
    let (from_f, to_f) = (fences(from), fences(to));
    let (src, dst) = (in_section(&from_f, anchor), in_section(&to_f, anchor));
    if src.len() != dst.len() {
        return None;
    }
    let (src, dst) = (src.get(index)?, dst.get(index)?);
    if src.lang != dst.lang || src.literal == dst.literal {
        return None;
    }
    let from_lines: Vec<&str> = from.split_inclusive('\n').collect();
    Some(splice(
        to,
        vec![(dst.inner.clone(), from_lines[src.inner.clone()].concat())],
    ))
}

/// Distinct section anchors of `f`, in order.
fn anchors(f: &[Fence]) -> Vec<&str> {
    let mut seen = HashSet::new();
    f.iter()
        .map(|x| x.anchor.as_str())
        .filter(|a| seen.insert(*a))
        .collect()
}

fn in_section<'a>(f: &'a [Fence], anchor: &str) -> Vec<&'a Fence> {
    f.iter().filter(|x| x.anchor == anchor).collect()
}

/// `text` with each line range replaced. Ranges must not overlap.
fn splice(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| r.start);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, replacement) in edits {
        out.push_str(&lines[at..range.start].concat());
        out.push_str(&replacement);
        at = range.end;
    }
    out.push_str(&lines[at..].concat());
    out
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
        let f = follow(OLD, NEW, other);
        assert_eq!(
            f.content.as_deref(),
            Some(
                "# Flow\n\n- dense: a to b\n\n~~~~ mermaid\ngraph TD\n  A-->C\n~~~~\n\n# Other\n\n```mermaid\nX\n```\n"
            )
        );
        assert_eq!((f.carried, f.skipped), (vec!["flow".to_owned()], vec![]));
    }

    #[test]
    fn leaves_diverged_missing_and_misplaced_copies() {
        let skipped = |other: &str| {
            let f = follow(OLD, NEW, other);
            assert_eq!(f.content, None, "{other}");
            assert!(f.carried.is_empty(), "{other}");
            f.skipped
        };
        let flow = vec!["flow".to_owned()];
        assert_eq!(
            skipped("# Flow\n\n```mermaid\ngraph TD\n  A-->Z\n```\n"),
            flow,
            "diverged"
        );
        assert_eq!(skipped("# Flow\n\nNo diagram.\n"), Vec::<String>::new());
        assert_eq!(
            skipped("# Other\n\n```mermaid\ngraph TD\n  A-->B\n```\n"),
            Vec::<String>::new(),
            "elsewhere"
        );
        let twice = "# Flow\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n```mermaid\ngraph TD\n  A-->B\n```\n";
        assert_eq!(skipped(twice), flow, "ambiguous copy");
    }

    #[test]
    fn ignores_added_and_removed_diagrams() {
        let added = "# Flow\n\n```mermaid\nnew\n```\n\n```mermaid\ngraph TD\n  A-->C\n```\n";
        let other = "# Flow\n\n```mermaid\ngraph TD\n  A-->B\n```\n";
        assert_eq!(follow(OLD, added, other), Follow::default());
        assert_eq!(follow(OLD, "# Flow\n\nGone.\n", other), Follow::default());
    }

    #[test]
    fn ignores_unclosed_and_nested_fences() {
        let old = "# F\n\n> ```mermaid\n> A\n> ```\n";
        let new = "# F\n\n> ```mermaid\n> B\n> ```\n";
        assert_eq!(follow(old, new, old), Follow::default());
        assert_eq!(
            follow(
                "# F\n\n```mermaid\nA\n",
                "# F\n\n```mermaid\nB\n",
                "# F\n\n```mermaid\nA\n"
            ),
            Follow::default()
        );
        assert!(drift(old, new).is_empty());
    }

    #[test]
    fn drift_pairs_diagrams_by_section_and_position() {
        let ai = "# Flow\n\n- a to b\n\n~~~~mermaid\ngraph TD\n  A-->B\n~~~~\n\n# Other\n\n```mermaid\nY\n```\n";
        assert!(drift(OLD, OLD).is_empty());
        assert_eq!(
            drift(NEW, ai),
            [
                Drift {
                    anchor: "flow".to_owned(),
                    index: 0,
                    lang: "mermaid".to_owned(),
                    human: "graph TD\n  A-->C\n".to_owned(),
                    ai: "graph TD\n  A-->B\n".to_owned(),
                },
                Drift {
                    anchor: "other".to_owned(),
                    index: 0,
                    lang: "mermaid".to_owned(),
                    human: "X\n".to_owned(),
                    ai: "Y\n".to_owned(),
                }
            ]
        );
        let extra = "# Flow\n\n```mermaid\nA\n```\n\n```mermaid\nB\n```\n";
        assert!(drift(NEW, extra).iter().all(|d| d.anchor != "flow"), "counts differ");
    }

    #[test]
    fn copy_replaces_one_diagram_and_keeps_fences() {
        let ai = "# Flow\n\n- a to b\n\n~~~~mermaid\ngraph TD\n  A-->B\n~~~~\n\n# Other\n\n```mermaid\nY\n```\n";
        assert_eq!(
            copy(NEW, ai, "flow", 0).as_deref(),
            Some("# Flow\n\n- a to b\n\n~~~~mermaid\ngraph TD\n  A-->C\n~~~~\n\n# Other\n\n```mermaid\nY\n```\n")
        );
        assert_eq!(
            copy(ai, NEW, "other", 0).as_deref(),
            Some(NEW.replace("```mermaid\nX\n", "```mermaid\nY\n").as_str())
        );
        assert_eq!(copy(NEW, NEW, "flow", 0), None, "already equal");
        assert_eq!(copy(NEW, ai, "flow", 1), None, "no such diagram");
        assert_eq!(copy(NEW, ai, "missing", 0), None);
    }
}
