//! Sync queue, per-document sync status, item detail (htmx), resolution.
//! Resolve form `anchor`: a section anchor, `*` for all pending sections, `+` for
//! pending sections present in both variants.

use serde::Deserialize;
use solidate_app::core::{DocPath, SyncState, Variant};
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::content::{Form, Html};
use topcoat::router::error::{SeeOther, bad_request, see_other};
use topcoat::router::{page, path_param_segment, path_param_segments, query_params, route};
use topcoat::view::{View, view};

use crate::auth::{app, tenant_ctx};
use crate::error::OrHttp;
use crate::ui::{action_url, diff_html, doc_url, enc, escape, project_url, tenant_url};

fn doc_path(cx: &Cx) -> Result<DocPath> {
    let joined = path_param_segments(cx, "path").collect::<Vec<_>>().join("/");
    DocPath::parse(&joined).map_err(|e| bad_request(e.to_string()).into())
}

pub fn state_class(s: SyncState) -> &'static str {
    match s {
        SyncState::InSync => "state ok",
        SyncState::Conflict => "state conflict",
        _ => "state stale",
    }
}

#[page("/t/{tenant}/p/{project}/sync")]
async fn queue(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let entries = app(cx).sync_queue(&ctx, project).await.or_http()?;
    let (t, p) = (ctx.tenant.slug.clone(), project.to_owned());
    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&t))>(ctx.tenant.name.as_str())</a>
            <a href=(project_url(&t, &p))>(p.clone())</a>
            <span>"Sync"</span>
        </nav>
        <h1>"Sync queue"</h1>
        <p class="muted">"Sections whose human and AI variants have diverged since they were last reconciled."</p>
        if entries.is_empty() {
            <p>"All sections are in sync."</p>
        } else {
            <table class="docs">
                <thead><tr><th>"Document"</th><th>"Section"</th><th>"State"</th><th>"Needs update"</th></tr></thead>
                <tbody>
                    for e in &entries {
                        <tr>
                            <td><a href=(format!("{}/sync/{}", project_url(&t, &p), e.path))><code>(e.path.as_str())</code></a></td>
                            <td>(if e.section_title.is_empty() { e.anchor.clone() } else { e.section_title.clone() })</td>
                            <td><span class=(state_class(e.state))>(e.state.as_str())</span></td>
                            <td>(e.stale_side.map_or("both (conflict)", |v| v.as_str()))</td>
                        </tr>
                    }
                </tbody>
            </table>
        }
    })
}

#[page("/t/{tenant}/p/{project}/sync/{*path}")]
async fn doc_sync(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let s = app(cx).doc_sync(&ctx, project, &path).await.or_http()?;
    let (t, p, ps) = (ctx.tenant.slug.clone(), project.to_owned(), path.as_str().to_owned());
    let base = project_url(&t, &p);
    let pending = s.sections.iter().filter(|x| x.state.needs_attention()).count();
    let paired = s
        .sections
        .iter()
        .filter(|x| x.state.needs_attention() && x.in_human && x.in_ai)
        .count();
    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&t))>(ctx.tenant.name.as_str())</a>
            <a href=(project_url(&t, &p))>(p.clone())</a>
            <a href=(doc_url(&t, &p, &ps, Variant::Human))>(ps.clone())</a>
            <span>"Sync"</span>
        </nav>
        <div class="head">
            <h1>(format!("Sync: {ps}"))</h1>
            <div class="actions">
                <a class="button secondary" href=(action_url(&t, &p, "edit", &ps, Variant::Human))>"Edit human"</a>
                <a class="button secondary" href=(action_url(&t, &p, "edit", &ps, Variant::Ai))>"Edit AI"</a>
            </div>
        </div>
        <form method="post" action=(format!("{base}/sync-enabled/{ps}")) class="inline">
            <input type="hidden" name="enabled" value=(if s.document.sync_enabled { "0" } else { "1" })>
            <span class="muted">(if s.document.sync_enabled { "Sync tracking is on. " } else { "Sync tracking is off. " })</span>
            <button type="submit" class="link">(if s.document.sync_enabled { "Turn off" } else { "Turn on" })</button>
        </form>
        <div class="actions bar">
            if paired > 0 && paired < pending {
                <form method="post" action=(format!("{base}/resolve/{ps}")) class="inline"
                      onsubmit="return confirm('Mark sections present in both variants as in sync without editing?')">
                    <input type="hidden" name="anchor" value="+">
                    <button type="submit" class="secondary">(format!("Mark paired in sync ({paired})"))</button>
                </form>
            }
            if pending > 0 {
                <form method="post" action=(format!("{base}/resolve/{ps}")) class="inline"
                      onsubmit="return confirm('Mark every section as in sync without editing?')">
                    <input type="hidden" name="anchor" value="*">
                    <button type="submit" class="secondary">(format!("Mark all in sync ({pending})"))</button>
                </form>
            }
        </div>
        <table class="docs">
            <thead><tr><th>"Section"</th><th>"State"</th><th></th></tr></thead>
            <tbody>
                for sec in &s.sections {
                    let detail_id = format!("item-{}", sec.anchor);
                    <tr>
                        <td>
                            (if sec.title.is_empty() { sec.anchor.clone() } else { sec.title.clone() })
                            " " <code class="muted small">(format!("#{}", sec.anchor))</code>
                        </td>
                        <td><span class=(state_class(sec.state))>(sec.state.as_str())</span></td>
                        <td class="actions">
                            if sec.state.needs_attention() {
                                match sec.stale_side {
                                    Some(v) => <a href=(propagate_url(&t, &p, &ps, v, &sec.anchor))>(if v == Variant::Ai { "Update AI" } else { "Update human" })</a>,
                                    None => "",
                                }
                                <button type="button" class="link"
                                    hx-get=(format!("{base}/sync-item/{ps}?anchor={}", enc(&sec.anchor)))
                                    hx-target=(format!("#{detail_id}")) hx-swap="innerHTML">"Details"</button>
                                <form method="post" action=(format!("{base}/resolve/{ps}")) class="inline">
                                    <input type="hidden" name="anchor" value=(sec.anchor.clone())>
                                    <button type="submit" class="link">"Mark in sync"</button>
                                </form>
                            }
                        </td>
                    </tr>
                    <tr class="detail"><td colspan="3" id=(detail_id)></td></tr>
                }
            </tbody>
        </table>
    })
}

#[query_params]
struct AnchorQuery {
    anchor: String,
}

/// URL of the editor for `v`, saving with `anchor` marked in sync.
pub fn propagate_url(t: &str, p: &str, path: &str, v: Variant, anchor: &str) -> String {
    let url = action_url(t, p, "edit", path, v);
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}resolves={}", enc(anchor))
}

/// Fragment with both sides' text, diffs since the last sync, and the base text
/// for conflicts.
#[route(GET "/t/{tenant}/p/{project}/sync-item/{*path}")]
async fn sync_item(cx: &Cx) -> Result<Html<String>> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let anchor = query_params::<AnchorQuery>(cx).map_err(|_| bad_request("anchor is required"))?;
    let item = app(cx)
        .sync_item(&ctx, project, &path, &anchor.anchor)
        .await
        .or_http()?;
    let (t, ps) = (ctx.tenant.slug.as_str(), path.as_str());
    let conflict = item.state == SyncState::Conflict;

    let mut html = String::from("<div class=\"sync-item\">");
    for (v, diff, text, base) in [
        (Variant::Human, &item.human_diff, &item.human, &item.human_base),
        (Variant::Ai, &item.ai_diff, &item.ai, &item.ai_base),
    ] {
        let label = if v == Variant::Human { "Human" } else { "AI" };
        html.push_str(&format!("<div><h3>{label}</h3>"));
        if let Some(d) = diff.as_deref().filter(|d| !d.is_empty()) {
            html.push_str("<p class=\"small muted\">Changes since last sync</p>");
            html.push_str(&diff_html(d).0);
        }
        match text {
            Some(t) => html.push_str(&format!("<pre class=\"source\">{}</pre>", escape(&t.body))),
            None => html.push_str("<p class=\"muted\">Section not present.</p>"),
        }
        if conflict && let Some(b) = base {
            html.push_str(&format!(
                "<details><summary class=\"small\">Text at last sync</summary><pre class=\"source\">{}</pre></details>",
                escape(b)
            ));
        }
        if conflict || item.stale_side == Some(v) {
            html.push_str(&format!(
                "<p><a class=\"button secondary\" href=\"{}\">Update {label}</a></p>",
                escape(&propagate_url(t, project, ps, v, &item.anchor))
            ));
        }
        html.push_str("</div>");
    }
    html.push_str("</div>");
    Ok(Html(html))
}

#[derive(Deserialize)]
struct ResolveForm {
    anchor: String,
}

#[route(POST "/t/{tenant}/p/{project}/resolve/{*path}")]
async fn resolve(cx: &Cx, Form(form): Form<ResolveForm>) -> Result<SeeOther> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let app = app(cx);
    if form.anchor == "+" {
        app.resolve_paired_sync(&ctx, project, &path).await.or_http()?;
        return Ok(see_other(format!(
            "{}/sync/{path}",
            project_url(&ctx.tenant.slug, project)
        )));
    }
    let anchors = if form.anchor == "*" {
        let s = app.doc_sync(&ctx, project, &path).await.or_http()?;
        s.sections
            .into_iter()
            .filter(|x| x.state.needs_attention())
            .map(|x| x.anchor)
            .collect()
    } else {
        vec![form.anchor]
    };
    app.resolve_sync(&ctx, project, &path, &anchors).await.or_http()?;
    Ok(see_other(format!(
        "{}/sync/{path}",
        project_url(&ctx.tenant.slug, project)
    )))
}

#[derive(Deserialize)]
struct EnabledForm {
    enabled: String,
}

#[route(POST "/t/{tenant}/p/{project}/sync-enabled/{*path}")]
async fn set_enabled(cx: &Cx, Form(form): Form<EnabledForm>) -> Result<SeeOther> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    app(cx)
        .set_doc_sync(&ctx, project, &path, form.enabled == "1")
        .await
        .or_http()?;
    Ok(see_other(format!(
        "{}/sync/{path}",
        project_url(&ctx.tenant.slug, project)
    )))
}
