//! View helpers: trusted HTML, URLs, formatting.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use solidate_app::core::{LinkTarget, Variant};
use time::OffsetDateTime;
use topcoat::context::Cx;
use topcoat::view::{NodeViewParts, PartsWriter};

/// HTML inserted without escaping. Only for output of the Markdown renderer (which
/// omits raw HTML) and other server-generated markup.
pub struct Trusted(pub String);

impl NodeViewParts for Trusted {
    fn into_view_parts(self, _cx: &Cx, parts: &mut PartsWriter<'_>) {
        parts.push_string_unescaped(self.0);
    }
}

pub fn enc(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn tenant_url(t: &str) -> String {
    format!("/t/{t}")
}

pub fn project_url(t: &str, p: &str) -> String {
    format!("/t/{t}/p/{p}")
}

/// Slugs and document paths contain only URL-safe characters.
pub fn doc_url(t: &str, p: &str, path: &str, v: Variant) -> String {
    match v {
        Variant::Human => format!("/t/{t}/p/{p}/d/{path}"),
        Variant::Ai => format!("/t/{t}/p/{p}/d/{path}?v=ai"),
    }
}

pub fn action_url(t: &str, p: &str, action: &str, path: &str, v: Variant) -> String {
    match v {
        Variant::Human => format!("/t/{t}/p/{p}/{action}/{path}"),
        Variant::Ai => format!("/t/{t}/p/{p}/{action}/{path}?v=ai"),
    }
}

/// URL for an internal link found in a document of `project`.
pub fn link_href(t: &str, project: &str, v: Variant, target: &LinkTarget) -> Option<String> {
    let p = target.project.as_ref().map_or(project, |s| s.as_str());
    let anchor = target.anchor.as_ref().map(|a| format!("#{a}")).unwrap_or_default();
    Some(match &target.path {
        Some(path) => format!("{}{anchor}", doc_url(t, p, path.as_str(), v)),
        None => anchor,
    })
}

pub fn parse_variant(v: Option<&str>) -> Variant {
    match v {
        Some("ai") => Variant::Ai,
        _ => Variant::Human,
    }
}

pub fn fmt_time(t: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

/// `url` with `carried` and `skipped` query parameters naming the sections whose
/// diagram edits a write carried into the AI variant or left alone. See
/// [`diagram_notices`].
pub fn with_diagram_report(mut url: String, carried: &[String], skipped: &[String]) -> String {
    for (key, anchors) in [("carried", carried), ("skipped", skipped)] {
        if anchors.is_empty() {
            continue;
        }
        let sep = if url.contains('?') { '&' } else { '?' };
        let value = anchors.iter().map(|a| enc(a)).collect::<Vec<_>>().join(",");
        url = format!("{url}{sep}{key}={value}");
    }
    url
}

/// Notices for the `carried` and `skipped` parameters of [`with_diagram_report`].
/// `sync_url` is the document's translation page, where differing diagrams are
/// reviewed.
pub fn diagram_notices(carried: Option<&str>, skipped: Option<&str>, sync_url: &str) -> Trusted {
    let list = |v: Option<&str>| -> Option<String> {
        let anchors: Vec<String> = v?
            .split(',')
            .filter(|a| !a.is_empty())
            .map(|a| format!("<code>#{}</code>", escape(a)))
            .collect();
        (!anchors.is_empty()).then(|| anchors.join(", "))
    };
    let mut out = String::new();
    if let Some(a) = list(carried) {
        out.push_str(&format!(
            "<p class=\"notice ok\">Diagram changes in {a} were also applied to the AI variant.</p>"
        ));
    }
    if let Some(a) = list(skipped) {
        out.push_str(&format!(
            "<p class=\"notice\">Diagram changes in {a} were not applied to the AI variant because its copy \
             differs. <a href=\"{}\">Choose a version</a>.</p>",
            escape(sync_url)
        ));
    }
    Trusted(out)
}

/// Notice for the `restored`, `paired` and `unpaired` parameters set after a restore
/// of variant `v`. `sync_url` is the document's translation page.
pub fn restore_notice(
    v: Variant,
    restored: Option<&str>,
    paired: Option<&str>,
    unpaired: Option<&str>,
    sync_url: &str,
) -> Trusted {
    let Some(rev) = restored.filter(|r| !r.is_empty()) else {
        return Trusted(String::new());
    };
    let list = |x: Option<&str>| -> Option<String> {
        let anchors: Vec<String> = x?
            .split(',')
            .filter(|a| !a.is_empty())
            .map(|a| format!("<code>#{}</code>", escape(a)))
            .collect();
        (!anchors.is_empty()).then(|| anchors.join(", "))
    };
    let label = |v: Variant| if v == Variant::Human { "human" } else { "AI" };
    let (v, o) = (label(v), label(v.other()));
    let mut out = format!(
        "<p class=\"notice ok\">Restored the {v} variant to revision <code>{}</code>.",
        escape(rev)
    );
    if let Some(a) = list(paired) {
        out.push_str(&format!(
            " The {o} variant's {a} went back to the text that was in sync."
        ));
    }
    out.push_str("</p>");
    if let Some(a) = list(unpaired) {
        out.push_str(&format!(
            "<p class=\"notice\">{a} in the {o} variant changed since the last sync and were not restored. \
             <a href=\"{}\">Review</a>.</p>",
            escape(sync_url)
        ));
    }
    Trusted(out)
}

/// Unified diff as HTML lines with `add`/`del`/`hunk` classes.
pub fn diff_html(diff: &str) -> Trusted {
    let mut out = String::from("<pre class=\"diff\">");
    for line in diff.lines() {
        let class = match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => "add",
            Some(b'-') if !line.starts_with("---") => "del",
            Some(b'@') => "hunk",
            _ => "ctx",
        };
        out.push_str(&format!("<span class=\"{class}\">{}</span>\n", escape(line)));
    }
    out.push_str("</pre>");
    Trusted(out)
}

/// A local redirect target: absolute path, no scheme or host.
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_owned(),
        _ => "/".to_owned(),
    }
}
