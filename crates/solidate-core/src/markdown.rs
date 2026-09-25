//! Markdown analysis and rendering on top of Comrak.
//!
//! Stored content is never rewritten. Normalization is used only for semantic hashes
//! (see [`crate::hash`]).
//!
//! A document splits into **sections** at every top-level heading, plus a preamble for
//! any text before the first heading. Each section has a stable **anchor**. It comes
//! from an explicit `{#id}` suffix on the heading when one is present, otherwise from
//! a slug of the heading text, de-duplicated within the document. Anchors pair a
//! document's human and AI sections for sync, and they're the `id`s in rendered HTML.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Mutex;

use comrak::adapters::{CodefenceRendererAdapter, HeadingAdapter, HeadingMeta};
use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use comrak::options::Plugins;
use comrak::{Arena, Options, format_html_with_plugins, html, markdown_to_commonmark, parse_document};
use serde::{Deserialize, Serialize};

use crate::hash::Hash;
use crate::links::{LinkTarget, parse_include, parse_markdown_link, parse_wiki_target};
use crate::path::DocPath;

/// The anchor of the text before a document's first heading.
pub const PREAMBLE_ANCHOR: &str = "_preamble";

/// The Comrak options Solidate uses for parsing and rendering: GFM plus wiki links,
/// footnotes and alerts. Raw HTML is never rendered.
pub fn options() -> Options<'static> {
    let mut o = Options::default();
    let e = &mut o.extension;
    e.strikethrough = true;
    e.table = true;
    e.autolink = true;
    e.tasklist = true;
    e.footnotes = true;
    e.alerts = true;
    e.wikilinks_title_after_pipe = true;
    o.render.r#unsafe = false;
    o
}

/// Canonical CommonMark for `md`. Formatting-only edits normalize to the same output.
pub fn normalize(md: &str) -> String {
    markdown_to_commonmark(md, &options())
}

pub fn semantic_hash(md: &str) -> Hash {
    Hash::of(normalize(md))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub anchor: String,
    /// Heading text without any `{#id}` suffix. Empty for the preamble.
    pub title: String,
    /// Heading level from 1 to 6, or 0 for the preamble.
    pub level: u8,
    /// Anchor of the closest enclosing heading with a lower level.
    pub parent: Option<String>,
    pub ordinal: u32,
    /// The section's raw source, including its heading line.
    pub body: String,
    /// Semantic hash of `body`.
    pub hash: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    pub content_hash: Hash,
    pub semantic_hash: Hash,
    /// The first level-1 heading, falling back to the first heading of any level.
    pub title: Option<String>,
    pub sections: Vec<Section>,
    /// Internal links, de-duplicated, in document order.
    pub links: Vec<LinkTarget>,
    /// Top-level `{{include ...}}` directives in document order.
    pub includes: Vec<LinkTarget>,
}

impl Analysis {
    pub fn section(&self, anchor: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.anchor == anchor)
    }

    /// The source of a section plus all of its subsections.
    pub fn section_tree_source(&self, anchor: &str) -> Option<String> {
        let start = self.sections.iter().position(|s| s.anchor == anchor)?;
        let level = self.sections[start].level;
        let mut out = self.sections[start].body.clone();
        for s in &self.sections[start + 1..] {
            if level == 0 || s.level <= level {
                break;
            }
            out.push_str(&s.body);
        }
        Some(out)
    }
}

/// Parses `md` and extracts everything Solidate stores about it. `base` is the
/// document's own path, used to resolve relative links.
pub fn analyze(md: &str, base: Option<&DocPath>) -> Analysis {
    let opts = options();
    let arena = Arena::new();
    let root = parse_document(&arena, md, &opts);
    let headings = assign_anchors(root);

    let lines: Vec<&str> = md.split_inclusive('\n').collect();
    let slice = |from: usize, to: usize| -> String {
        // 1-based and inclusive, clamped to the document.
        let to = to.min(lines.len());
        if from == 0 || from > to {
            String::new()
        } else {
            lines[from - 1..to].concat()
        }
    };

    let top: Vec<&AnchoredHeading> = headings.iter().filter(|h| h.top_level).collect();
    let mut sections = Vec::with_capacity(top.len() + 1);
    let first_line = top.first().map_or(lines.len() + 1, |h| h.sourcepos.start.line);
    let preamble = slice(1, first_line - 1);
    if !preamble.trim().is_empty() {
        sections.push(section(PREAMBLE_ANCHOR, "", 0, None, 0, preamble));
    }
    let mut stack: Vec<(u8, String)> = Vec::new();
    for (i, h) in top.iter().enumerate() {
        let end = top.get(i + 1).map_or(lines.len(), |n| n.sourcepos.start.line - 1);
        while stack.last().is_some_and(|(lvl, _)| *lvl >= h.level) {
            stack.pop();
        }
        let parent = stack.last().map(|(_, a)| a.clone());
        let ordinal = sections.len() as u32;
        sections.push(section(
            &h.anchor,
            &h.title,
            h.level,
            parent,
            ordinal,
            slice(h.sourcepos.start.line, end),
        ));
        stack.push((h.level, h.anchor.clone()));
    }

    let title = headings
        .iter()
        .find(|h| h.level == 1)
        .or(headings.first())
        .map(|h| h.title.clone());

    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for node in root.descendants() {
        let target = match &node.data().value {
            NodeValue::Link(l) => parse_markdown_link(&l.url, base),
            NodeValue::WikiLink(w) => parse_wiki_target(&w.url),
            _ => None,
        };
        if let Some(t) = target.filter(|t| seen.insert(t.clone())) {
            links.push(t);
        }
    }

    let includes = top_level_includes(root).into_iter().map(|(t, _)| t).collect();

    Analysis {
        content_hash: Hash::of(md),
        semantic_hash: semantic_hash(md),
        title,
        sections,
        links,
        includes,
    }
}

fn section(anchor: &str, title: &str, level: u8, parent: Option<String>, ordinal: u32, body: String) -> Section {
    Section {
        anchor: anchor.to_owned(),
        title: title.to_owned(),
        level,
        parent,
        ordinal,
        hash: semantic_hash(&body),
        body,
    }
}

/// Top-level paragraphs that consist solely of an include directive, with their
/// 1-based inclusive line ranges.
pub(crate) fn top_level_includes<'a>(root: &'a AstNode<'a>) -> Vec<(LinkTarget, (usize, usize))> {
    root.children()
        .filter(|n| matches!(n.data().value, NodeValue::Paragraph))
        .filter_map(|n| {
            let target = parse_include(&n.collect_text())?;
            let sp = n.data().sourcepos;
            Some((target, (sp.start.line, sp.end.line)))
        })
        .collect()
}

struct AnchoredHeading {
    level: u8,
    title: String,
    anchor: String,
    top_level: bool,
    sourcepos: Sourcepos,
}

/// Gives every heading an anchor, in document order, and strips explicit `{#id}`
/// suffixes from the AST so they don't render.
fn assign_anchors<'a>(root: &'a AstNode<'a>) -> Vec<AnchoredHeading> {
    let mut used: HashSet<String> = HashSet::from([PREAMBLE_ANCHOR.to_owned()]);
    let mut out = Vec::new();
    for node in root.descendants() {
        let level = match &node.data().value {
            NodeValue::Heading(h) => h.level,
            _ => continue,
        };
        let text = node.collect_text();
        let (title, explicit) = split_explicit_anchor(&text);
        let title = title.to_owned();
        if explicit.is_some() {
            strip_anchor_suffix(node);
        }
        let base = explicit.map_or_else(|| slugify(&title), str::to_owned);
        out.push(AnchoredHeading {
            level,
            anchor: unique(&mut used, &base),
            title,
            top_level: node.parent().is_some_and(|p| std::ptr::eq(p, root)),
            sourcepos: node.data().sourcepos,
        });
    }
    out
}

fn unique(used: &mut HashSet<String>, base: &str) -> String {
    let base = if base.is_empty() { "section" } else { base };
    if used.insert(base.to_owned()) {
        return base.to_owned();
    }
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| used.insert(c.clone()))
        .expect("unbounded")
}

/// Lowercases `text` and keeps ASCII letters, digits, `_` and single `-` separators.
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '_' {
            out.push(c);
        } else if (c.is_whitespace() || c == '-') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Splits `Title {#id}` into `("Title", Some("id"))`.
fn split_explicit_anchor(text: &str) -> (&str, Option<&str>) {
    let t = text.trim_end();
    if let Some(inner) = t.strip_suffix('}')
        && let Some(i) = inner.rfind("{#")
    {
        let id = &inner[i + 2..];
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return (t[..i].trim(), Some(id));
        }
    }
    (text.trim(), None)
}

fn strip_anchor_suffix<'a>(heading: &'a AstNode<'a>) {
    let mut texts = Vec::new();
    let mut cur = heading.last_child();
    while let Some(n) = cur {
        if !matches!(n.data().value, NodeValue::Text(_)) {
            break;
        }
        texts.push(n);
        cur = n.previous_sibling();
    }
    texts.reverse();
    let combined: String = texts
        .iter()
        .filter_map(|n| n.data().value.text().map(str::to_owned))
        .collect();
    let t = combined.trim_end();
    let Some(i) = t.rfind("{#") else { return };
    let kept = t[..i].trim_end().to_owned();
    if let Some((first, rest)) = texts.split_first() {
        if let Some(text) = first.data_mut().value.text_mut() {
            *text = kept.into();
        }
        for n in rest {
            n.detach();
        }
    }
}

/// Rendered HTML plus the heading outline, for a table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub html: String,
    pub outline: Vec<OutlineEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutlineEntry {
    pub level: u8,
    pub title: String,
    pub anchor: String,
}

/// Renders `md` to HTML that's safe to embed: raw HTML is omitted and headings get
/// their section anchors as `id`s. `href` maps each internal link target to a URL.
/// Links it returns `None` for are left unchanged. `mermaid` code fences render as
/// `<pre class="mermaid">` with escaped source, for client-side diagram rendering.
pub fn render_html(md: &str, base: Option<&DocPath>, href: &dyn Fn(&LinkTarget) -> Option<String>) -> Rendered {
    let opts = options();
    let arena = Arena::new();
    let root = parse_document(&arena, md, &opts);
    let headings = assign_anchors(root);

    for node in root.descendants() {
        let mut data = node.data_mut();
        match &mut data.value {
            NodeValue::Link(l) => {
                if let Some(url) = parse_markdown_link(&l.url, base).as_ref().and_then(href) {
                    l.url = url;
                }
            }
            NodeValue::WikiLink(w) => {
                if let Some(url) = parse_wiki_target(&w.url).as_ref().and_then(href) {
                    w.url = url;
                }
            }
            _ => {}
        }
    }

    let adapter = AnchoredHeadings(Mutex::new(headings.iter().map(|h| h.anchor.clone()).collect()));
    let mut plugins = Plugins::default();
    plugins.render.heading_adapter = Some(&adapter);
    plugins
        .render
        .codefence_renderers
        .insert("mermaid".to_owned(), &Mermaid);
    let mut html = String::new();
    format_html_with_plugins(root, &opts, &mut html, &plugins).expect("writing to a String cannot fail");

    let outline = headings
        .into_iter()
        .map(|h| OutlineEntry {
            level: h.level,
            title: h.title,
            anchor: h.anchor,
        })
        .collect();
    Rendered { html, outline }
}

/// Emits pre-computed anchors as heading ids. Comrak renders headings in document
/// order, the same order [`assign_anchors`] visits them.
struct AnchoredHeadings(Mutex<VecDeque<String>>);

impl HeadingAdapter for AnchoredHeadings {
    fn enter(&self, out: &mut dyn fmt::Write, h: &HeadingMeta, _: Option<Sourcepos>) -> fmt::Result {
        let anchor = self.0.lock().expect("not poisoned").pop_front().unwrap_or_default();
        // Anchors contain only characters that are safe in attributes.
        write!(
            out,
            "<h{} id=\"{anchor}\"><a class=\"anchor\" href=\"#{anchor}\" aria-hidden=\"true\">#</a>",
            h.level
        )
    }

    fn exit(&self, out: &mut dyn fmt::Write, h: &HeadingMeta) -> fmt::Result {
        writeln!(out, "</h{}>", h.level)
    }
}

/// Emits `mermaid` code fences as `<pre class="mermaid">` for the client-side renderer.
struct Mermaid;

impl CodefenceRendererAdapter for Mermaid {
    fn write(&self, out: &mut dyn fmt::Write, _: &str, _: &str, code: &str, _: Option<Sourcepos>) -> fmt::Result {
        out.write_str("<pre class=\"mermaid\">")?;
        html::escape(out, code)?;
        out.write_str("</pre>\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "Intro text.\n\n# Auth {#auth}\n\nOverview.\n\n## Tokens\n\nSee [sessions](sessions#ttl) and [[shared:rules/style#tone|style]].\n\n## Tokens\n\nDupe.\n\n{{include shared:rules#security}}\n\n```md\n# not a heading\n```\n";

    #[test]
    fn splits_sections_with_stable_anchors() {
        let a = analyze(DOC, Some(&DocPath::parse("design/auth").unwrap()));
        let anchors: Vec<_> = a.sections.iter().map(|s| s.anchor.as_str()).collect();
        assert_eq!(anchors, ["_preamble", "auth", "tokens", "tokens-1"]);
        assert_eq!(a.title.as_deref(), Some("Auth"));
        assert_eq!(a.sections[1].title, "Auth");
        assert_eq!(a.sections[2].parent.as_deref(), Some("auth"));
        assert!(a.sections[3].body.contains("# not a heading"));
        assert_eq!(a.sections.iter().map(|s| s.body.as_str()).collect::<String>(), DOC);
    }

    #[test]
    fn extracts_links_and_includes() {
        let a = analyze(DOC, Some(&DocPath::parse("design/auth").unwrap()));
        let links: Vec<_> = a.links.iter().map(ToString::to_string).collect();
        assert_eq!(links, ["design/sessions#ttl", "shared:rules/style#tone"]);
        assert_eq!(a.includes.len(), 1);
        assert_eq!(a.includes[0].to_string(), "shared:rules#security");
    }

    #[test]
    fn semantic_hash_ignores_formatting_but_content_hash_does_not() {
        let a = "# Title\n\nSome *text* here\n* one\n* two\n";
        let b = "Title\n=====\n\nSome _text_ here\n\n- one\n- two\n";
        assert_eq!(semantic_hash(a), semantic_hash(b));
        assert_ne!(Hash::of(a), Hash::of(b));
        assert_ne!(semantic_hash(a), semantic_hash("# Title\n\nOther text\n"));
    }

    #[test]
    fn section_tree_includes_subsections() {
        let a = analyze(DOC, None);
        let tree = a.section_tree_source("auth").unwrap();
        assert!(tree.starts_with("# Auth") && tree.contains("Dupe."));
        assert_eq!(
            a.section_tree_source("tokens").unwrap(),
            a.section("tokens").unwrap().body
        );
    }

    #[test]
    fn renders_safe_html_with_anchor_ids_and_rewritten_links() {
        let r = render_html(
            "# Hello {#hi}\n\n<script>x</script>\n\n[[other]] [ext](https://e.x)\n",
            None,
            &|t| Some(format!("/d/{t}")),
        );
        assert!(
            r.html.contains(r##"<h1 id="hi"><a class="anchor" href="#hi""##),
            "{}",
            r.html
        );
        assert!(r.html.contains("Hello</h1>") && !r.html.contains("{#hi}"));
        assert!(!r.html.contains("<script>"));
        assert!(r.html.contains(r#"href="/d/other""#));
        assert!(r.html.contains(r#"href="https://e.x""#));
        assert_eq!(
            r.outline,
            vec![OutlineEntry {
                level: 1,
                title: "Hello".into(),
                anchor: "hi".into()
            }]
        );
    }

    #[test]
    fn renders_mermaid_fences_as_escaped_pre() {
        let r = render_html(
            "```mermaid\ngraph TD\n  A-->B<script>\n```\n\n```rust\nfn x() {}\n```\n",
            None,
            &|_| None,
        );
        assert!(
            r.html
                .contains("<pre class=\"mermaid\">graph TD\n  A--&gt;B&lt;script&gt;\n</pre>"),
            "{}",
            r.html
        );
        assert!(r.html.contains(r#"<code class="language-rust">"#));
    }

    #[test]
    fn slugify_rules() {
        assert_eq!(slugify("  Hello, World! -- v2 "), "hello-world-v2");
        assert_eq!(slugify("snake_case"), "snake_case");
        assert_eq!(slugify("!!!"), "");
    }
}
