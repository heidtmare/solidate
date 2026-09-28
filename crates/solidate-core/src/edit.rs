//! Section-level edits: replace, delete or insert one top-level section of a
//! document and leave the rest of the source byte-for-byte unchanged.
//!
//! Section boundaries are the ones [`analyze`] produces. A splice is rejected when it
//! would change the anchors of sections outside the edited span, or when inserted text
//! without a leading heading would merge into the preceding section.

use crate::hash::Hash;
use crate::markdown::{Analysis, PREAMBLE_ANCHOR, analyze, semantic_hash};

/// Where a section edit applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionTarget<'a> {
    /// The section with this anchor. With `subsections`, the span also covers the
    /// following sections of a deeper level. Empty content deletes the span.
    Section { anchor: &'a str, subsections: bool },
    /// After the section with this anchor and its subsections.
    After(&'a str),
    /// At the end of the document.
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    /// The full document after the edit.
    pub content: String,
    /// Semantic hash of the replaced span. `None` for insertions.
    pub replaced_hash: Option<Hash>,
    /// Anchors of the sections in the written span, in document order.
    pub anchors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpliceError {
    #[error("no section #{0}")]
    NoSection(String),
    #[error("content must start with a heading; text before it would join the preceding section")]
    NoHeading,
    #[error("the edit would change or remove the anchors of other sections: {}", .0.join(", "))]
    AnchorsChanged(Vec<String>),
}

/// Semantic hash of the span `target` names in `a`: one section, or a section and
/// its subsections. `None` when there is no such section or `target` is an insertion.
pub fn span_hash(a: &Analysis, target: SectionTarget<'_>) -> Option<Hash> {
    let SectionTarget::Section { anchor, subsections } = target else {
        return None;
    };
    if subsections {
        a.section_tree_source(anchor).map(|s| semantic_hash(&s))
    } else {
        a.section(anchor).map(|s| s.hash)
    }
}

/// Applies `content` at `target` in `md`.
pub fn splice_section(md: &str, target: SectionTarget<'_>, content: &str) -> Result<Splice, SpliceError> {
    let a = analyze(md, None);
    let offsets = section_offsets(md, &a);
    let find = |anchor: &str| {
        a.sections
            .iter()
            .position(|s| s.anchor == anchor)
            .ok_or_else(|| SpliceError::NoSection(anchor.to_owned()))
    };
    // Section index range [from, to) covered by the span.
    let (from, to) = match target {
        SectionTarget::Section { anchor, subsections } => {
            let i = find(anchor)?;
            (i, if subsections { subtree_end(&a, i) } else { i + 1 })
        }
        SectionTarget::After(anchor) => {
            let end = subtree_end(&a, find(anchor)?);
            (end, end)
        }
        SectionTarget::End => (a.sections.len(), a.sections.len()),
    };
    let start = offsets.get(from).copied().unwrap_or(md.len());
    let end = offsets.get(to).copied().unwrap_or(md.len());

    let body = content.trim_end_matches(['\n', '\r']);
    let (prefix, suffix) = (&md[..start], &md[end..]);
    if !body.is_empty() && !prefix.trim().is_empty() {
        let first = analyze(body, None);
        if first.sections.first().is_none_or(|s| s.anchor == PREAMBLE_ANCHOR) {
            return Err(SpliceError::NoHeading);
        }
    }

    let mut out = String::with_capacity(prefix.len() + body.len() + suffix.len() + 4);
    out.push_str(prefix);
    let written_start = if body.is_empty() {
        out.len()
    } else {
        // Keep a blank line between the inserted text and its neighbours, so a setext
        // heading or lazy continuation cannot merge across the boundary.
        if !prefix.is_empty() && !prefix.ends_with("\n\n") {
            out.push_str(if prefix.ends_with('\n') { "\n" } else { "\n\n" });
        }
        let s = out.len();
        out.push_str(body);
        out.push('\n');
        if !suffix.is_empty() {
            out.push('\n');
        }
        s
    };
    let written_end = out.len();
    out.push_str(suffix);

    let b = analyze(&out, None);
    let anchors: Vec<String> = b
        .sections
        .iter()
        .zip(section_offsets(&out, &b))
        .filter(|(_, o)| (written_start..written_end).contains(o))
        .map(|(s, _)| s.anchor.clone())
        .collect();
    // Every section outside the span must keep its anchor. It is lost when its
    // heading was swallowed (e.g. by an unclosed fence) or when a written section
    // took the anchor and pushed it to a de-duplicated one.
    let lost: Vec<String> = a.sections[..from]
        .iter()
        .chain(&a.sections[to..])
        .filter(|s| b.section(&s.anchor).is_none() || anchors.contains(&s.anchor))
        .map(|s| s.anchor.clone())
        .collect();
    if !lost.is_empty() {
        return Err(SpliceError::AnchorsChanged(lost));
    }
    Ok(Splice {
        content: out,
        replaced_hash: span_hash(&a, target),
        anchors,
    })
}

/// Byte offset at which each section of `a` starts in `md`. Sections are contiguous
/// from the first one to the end of the document.
fn section_offsets(md: &str, a: &Analysis) -> Vec<usize> {
    let mut rest: usize = a.sections.iter().map(|s| s.body.len()).sum();
    a.sections
        .iter()
        .map(|s| {
            let at = md.len() - rest;
            rest -= s.body.len();
            at
        })
        .collect()
}

/// Index one past the last subsection of section `i`.
fn subtree_end(a: &Analysis, i: usize) -> usize {
    let level = a.sections[i].level;
    if level == 0 {
        return i + 1;
    }
    a.sections[i + 1..]
        .iter()
        .position(|s| s.level <= level)
        .map_or(a.sections.len(), |n| i + 1 + n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "Intro.\n\n# Auth\n\nOverview.\n\n## Tokens\n\nOld.\n\n## Sessions\n\nTTL.\n\n# Other\n\nText.\n";

    fn replace(anchor: &str, subsections: bool) -> SectionTarget<'_> {
        SectionTarget::Section { anchor, subsections }
    }

    #[test]
    fn replaces_one_section_and_keeps_the_rest() {
        let s = splice_section(DOC, replace("tokens", false), "## Tokens\n\nNew.").unwrap();
        assert_eq!(s.content, DOC.replace("Old.", "New."));
        assert_eq!(s.anchors, ["tokens"]);
        assert_eq!(s.replaced_hash, Some(semantic_hash("## Tokens\n\nOld.\n\n")));
    }

    #[test]
    fn replaces_subtree_and_last_section() {
        let s = splice_section(DOC, replace("auth", true), "# Auth\n\nAll new.\n").unwrap();
        assert_eq!(s.content, "Intro.\n\n# Auth\n\nAll new.\n\n# Other\n\nText.\n");
        let s = splice_section(DOC, replace("other", false), "# Other\n\nEnd.\n\n\n").unwrap();
        assert!(s.content.ends_with("TTL.\n\n# Other\n\nEnd.\n"));
    }

    #[test]
    fn deletes_and_inserts() {
        let s = splice_section(DOC, replace("sessions", false), "").unwrap();
        assert_eq!(s.content, DOC.replace("## Sessions\n\nTTL.\n\n", ""));
        assert!(s.anchors.is_empty());

        let s = splice_section(DOC, SectionTarget::After("auth"), "# Middle\nM.").unwrap();
        assert!(s.content.contains("TTL.\n\n# Middle\nM.\n\n# Other"), "{}", s.content);
        assert_eq!(s.anchors, ["middle"]);
        assert_eq!(s.replaced_hash, None);

        let s = splice_section("# A\nText", SectionTarget::End, "# B\n").unwrap();
        assert_eq!(s.content, "# A\nText\n\n# B\n");
        let s = splice_section("", SectionTarget::End, "Just text.").unwrap();
        assert_eq!(s.content, "Just text.\n");
    }

    #[test]
    fn replaces_preamble() {
        let s = splice_section(DOC, replace("_preamble", true), "New intro.").unwrap();
        assert!(s.content.starts_with("New intro.\n\n# Auth\n"));
        assert_eq!(s.anchors, ["_preamble"]);
    }

    #[test]
    fn rejects_unsafe_splices() {
        assert_eq!(
            splice_section(DOC, replace("nope", false), "# X\n"),
            Err(SpliceError::NoSection("nope".into()))
        );
        assert_eq!(
            splice_section(DOC, SectionTarget::After("auth"), "no heading"),
            Err(SpliceError::NoHeading)
        );
        // An unclosed fence swallows the following sections.
        assert!(matches!(
            splice_section(DOC, replace("tokens", false), "## Tokens\n\n```\ncode"),
            Err(SpliceError::AnchorsChanged(a)) if a == ["sessions", "other"]
        ));
        // A duplicate heading shifts a later anchor.
        assert!(matches!(
            splice_section(DOC, replace("tokens", false), "## Sessions\n"),
            Err(SpliceError::AnchorsChanged(_))
        ));
    }

    #[test]
    fn span_hash_matches_read_hashes() {
        let a = analyze(DOC, None);
        assert_eq!(
            span_hash(&a, replace("tokens", false)),
            Some(a.section("tokens").unwrap().hash)
        );
        assert_eq!(
            span_hash(&a, replace("auth", true)),
            Some(semantic_hash(&a.section_tree_source("auth").unwrap()))
        );
        assert_eq!(span_hash(&a, SectionTarget::End), None);
    }
}
