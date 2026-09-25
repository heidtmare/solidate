//! Translation between a document's human and AI variants: sync queue, per-document
//! status with proposal review, item detail (htmx), resolution. Resolve form
//! `anchor`: a section anchor, `*` for all pending sections, `+` for pending sections
//! present in both variants.

use serde::Deserialize;
use solidate_app::core::{DocPath, SyncState, Variant};
use solidate_app::db::ProposalId;
use solidate_app::{GUIDE_PATH, ProposalRef};
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::content::{Form, Html};
use topcoat::router::error::{SeeOther, bad_request, see_other};
use topcoat::router::{page, path_param_segment, path_param_segments, query_params, route};
use topcoat::view::{View, view};

use crate::auth::{app, tenant_ctx};
use crate::error::OrHttp;
use crate::ui::{action_url, diff_html, doc_url, enc, escape, fmt_time, project_url, tenant_url};

fn doc_path(cx: &Cx) -> Result<DocPath> {
    let joined = path_param_segments(cx, "path").collect::<Vec<_>>().join("/");
    DocPath::parse(&joined).map_err(|e| bad_request(e.to_string()).into())
}

pub fn state_label(s: SyncState) -> &'static str {
    match s {
        SyncState::InSync => "in sync",
        SyncState::HumanAhead => "human changed",
        SyncState::AiAhead => "AI changed",
        SyncState::Conflict => "both changed",
    }
}

fn proposal_label(p: Option<ProposalRef>) -> &'static str {
    match p {
        Some(p) if p.outdated => "outdated",
        Some(_) => "awaiting review",
        None => "",
    }
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
    let proposals = app(cx).project_proposals(&ctx, project).await.or_http()?;
    let (t, p) = (ctx.tenant.slug.clone(), project.to_owned());
    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&t))>(ctx.tenant.name.as_str())</a>
            <a href=(project_url(&t, &p))>(p.clone())</a>
            <span>"Sync"</span>
        </nav>
        <h1>"Translation queue"</h1>
        <p class="muted">
            "Each document states one truth twice: a human variant and an AI variant that translate each other section by section. "
            "These sections changed in one variant and await translation into the other. Agents translate them and submit proposals for review here."
        </p>
        if !proposals.is_empty() {
            <h2>(format!("Proposals awaiting review ({})", proposals.len()))</h2>
            <ul class="plain">
                for pr in &proposals {
                    <li>
                        <a href=(format!("{}/sync/{}", project_url(&t, &p), pr.proposal.path))><code>(pr.proposal.path.clone())</code></a>
                        " " (format!("into {}", pr.proposal.variant))
                        if pr.outdated { " " <span class="state stale">"outdated"</span> }
                    </li>
                }
            </ul>
        }
        if entries.is_empty() {
            <p>"All sections are in sync."</p>
        } else {
            <table class="docs">
                <thead><tr><th>"Document"</th><th>"Section"</th><th>"State"</th><th>"Translate into"</th><th>"Proposal"</th></tr></thead>
                <tbody>
                    for e in &entries {
                        <tr>
                            <td><a href=(format!("{}/sync/{}", project_url(&t, &p), e.path))><code>(e.path.as_str())</code></a></td>
                            <td>(if e.section_title.is_empty() { e.anchor.clone() } else { e.section_title.clone() })</td>
                            <td><span class=(state_class(e.state))>(state_label(e.state))</span></td>
                            <td>(e.stale_side.map_or("both (reconcile)", |v| v.as_str()))</td>
                            <td>(proposal_label(e.proposal))</td>
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
    let proposals = app(cx).doc_proposals(&ctx, project, &path).await.or_http()?;
    let guide = app(cx).translation_guide(&ctx, project).await.or_http()?;
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
            <h1>(format!("Translation: {ps}"))</h1>
            <div class="actions">
                <a class="button secondary" href=(action_url(&t, &p, "edit", &ps, Variant::Human))>"Edit human"</a>
                <a class="button secondary" href=(action_url(&t, &p, "edit", &ps, Variant::Ai))>"Edit AI"</a>
            </div>
        </div>
        <p class="muted">
            "The human and AI variants translate each other. When a section changes in one, an agent translates it into the other and proposes the result below. "
            "Use \u{201c}No translation needed\u{201d} only when a change does not alter meaning, such as a typo fix."
        </p>
        <p class="small">
            "Translation guide: "
            match &guide.source {
                Some(src) => {
                    let (owner, gpath) = src.split_once(':').unwrap_or((src.as_str(), GUIDE_PATH));
                    <a href=(doc_url(&t, owner, gpath, Variant::Human))><code>(src.clone())</code></a>
                },
                None => {
                    "built-in default ("
                    <a href=(action_url(&t, &p, "edit", GUIDE_PATH, Variant::Human))>(format!("write {GUIDE_PATH}"))</a>
                    " to customize)"
                },
            }
        </p>
        for pr in &proposals {
            <section class="proposal">
                <div class="head">
                    <h2>
                        (format!("Proposed {} translation", pr.proposal.variant))
                        if pr.outdated { " " <span class="state stale">"outdated"</span> }
                    </h2>
                    <div class="actions">
                        if !pr.outdated {
                            <form method="post" action=(format!("{base}/proposal/{}/accept", pr.proposal.id)) class="inline">
                                <button type="submit">"Accept"</button>
                            </form>
                        }
                        <form method="post" action=(format!("{base}/proposal/{}/reject", pr.proposal.id)) class="inline">
                            <button type="submit" class="secondary">"Reject"</button>
                        </form>
                    </div>
                </div>
                <p class="small muted">
                    (match (pr.proposal.author_user_id, pr.proposal.author_token_id) {
                        (Some(_), _) => "By a user",
                        (_, Some(_)) => "By an agent (API token)",
                        _ => "By the system",
                    })
                    (format!(" \u{b7} {}", fmt_time(pr.proposal.created_at)))
                    if !pr.proposal.resolves.is_empty() {
                        (format!(" \u{b7} covers {}", pr.proposal.resolves.iter().map(|a| format!("#{a}")).collect::<Vec<_>>().join(", ")))
                    }
                </p>
                if pr.outdated {
                    <p class="notice">"A variant changed after this was proposed. Reject it; the agent will translate the current text."</p>
                }
                match &pr.proposal.message {
                    Some(m) => <p>(m.clone())</p>,
                    None => "",
                }
                (diff_html(&pr.diff))
            </section>
        }
        <form method="post" action=(format!("{base}/sync-enabled/{ps}")) class="inline">
            <input type="hidden" name="enabled" value=(if s.document.sync_enabled { "0" } else { "1" })>
            <span class="muted">(if s.document.sync_enabled { "Translation tracking is on. " } else { "Translation tracking is off. " })</span>
            <button type="submit" class="link">(if s.document.sync_enabled { "Turn off" } else { "Turn on" })</button>
        </form>
        <div class="actions bar">
            if paired > 0 && paired < pending {
                <form method="post" action=(format!("{base}/resolve/{ps}")) class="inline"
                      onsubmit="return confirm('Accept the current text of every section present in both variants as equivalent, without translating?')">
                    <input type="hidden" name="anchor" value="+">
                    <button type="submit" class="secondary">(format!("No translation needed: paired sections ({paired})"))</button>
                </form>
            }
            if pending > 0 {
                <form method="post" action=(format!("{base}/resolve/{ps}")) class="inline"
                      onsubmit="return confirm('Accept the current text of every section as equivalent, without translating?')">
                    <input type="hidden" name="anchor" value="*">
                    <button type="submit" class="secondary">(format!("No translation needed: all ({pending})"))</button>
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
                        <td>
                            <span class=(state_class(sec.state))>(state_label(sec.state))</span>
                            match proposal_label(sec.proposal) {
                                "" => "",
                                l => <span class="badge">(format!("proposal {l}"))</span>,
                            }
                        </td>
                        <td class="actions">
                            if sec.state.needs_attention() {
                                match sec.stale_side {
                                    Some(v) => <a href=(propagate_url(&t, &p, &ps, v, &sec.anchor))>(format!("Translate into {v} by hand"))</a>,
                                    None => "",
                                }
                                <button type="button" class="link"
                                    hx-get=(format!("{base}/sync-item/{ps}?anchor={}", enc(&sec.anchor)))
                                    hx-target=(format!("#{detail_id}")) hx-swap="innerHTML">"Details"</button>
                                <form method="post" action=(format!("{base}/resolve/{ps}")) class="inline">
                                    <input type="hidden" name="anchor" value=(sec.anchor.clone())>
                                    <button type="submit" class="link">"No translation needed"</button>
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
                "<p><a class=\"button secondary\" href=\"{}\">Translate into {label} by hand</a></p>",
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

fn proposal_id(cx: &Cx) -> Result<ProposalId> {
    path_param_segment(cx, "id")
        .parse()
        .map_err(|_| bad_request("invalid proposal id").into())
}

/// Accepts a proposal and returns to its document's translation page.
#[route(POST "/t/{tenant}/p/{project}/proposal/{id}/accept")]
async fn accept(cx: &Cx) -> Result<SeeOther> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let r = app(cx)
        .accept_proposal(&ctx, project, proposal_id(cx)?)
        .await
        .or_http()?;
    Ok(see_other(format!(
        "{}/sync/{}",
        project_url(&ctx.tenant.slug, project),
        r.document.path
    )))
}

#[route(POST "/t/{tenant}/p/{project}/proposal/{id}/reject")]
async fn reject(cx: &Cx) -> Result<SeeOther> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let path = app(cx)
        .reject_proposal(&ctx, project, proposal_id(cx)?)
        .await
        .or_http()?;
    Ok(see_other(format!(
        "{}/sync/{path}",
        project_url(&ctx.tenant.slug, project)
    )))
}
