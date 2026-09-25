//! Document view, editor, creation, history, deletion, and Markdown preview.

use serde::Deserialize;
use solidate_app::core::{DocPath, Hash, SyncState, Variant, render_html};
use solidate_app::db::Expect;
use solidate_app::{AppError, PutDoc};
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::content::{Form, Html};
use topcoat::router::error::{SeeOther, bad_request, not_found, see_other};
use topcoat::router::{StatusCode, page, path_param_segment, path_param_segments, query_params, route};
use topcoat::view::{View, component, view};

use crate::auth::{app, require_user, tenant_ctx};
use crate::error::{OrHttp, http};
use crate::ui::{Trusted, action_url, diff_html, doc_url, fmt_time, link_href, parse_variant, project_url, tenant_url};

fn doc_path(cx: &Cx) -> Result<DocPath> {
    let joined = path_param_segments(cx, "path").collect::<Vec<_>>().join("/");
    DocPath::parse(&joined).map_err(|e| bad_request(e.to_string()).into())
}

#[query_params]
struct VariantQuery {
    v: Option<String>,
    /// Section anchor the edit propagates; saving marks it in sync.
    resolves: Option<String>,
}

fn variant(cx: &Cx) -> Variant {
    parse_variant(query_params::<VariantQuery>(cx).ok().and_then(|q| q.v.as_deref()))
}

fn resolves_param(cx: &Cx) -> Option<String> {
    query_params::<VariantQuery>(cx)
        .ok()
        .and_then(|q| q.resolves.clone())
        .filter(|a| !a.is_empty())
}

#[component]
async fn crumbs(
    tenant: String,
    tenant_name: String,
    project: String,
    #[default] path: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&tenant))>(tenant_name)</a>
            <a href=(project_url(&tenant, &project))>(project.clone())</a>
            match path {
                Some(p) => <span>(p)</span>,
                None => "",
            }
        </nav>
    })
}

#[page("/t/{tenant}/p/{project}/d/{*path}")]
async fn doc_view(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path, v) = (path_param_segment(cx, "project"), doc_path(cx)?, variant(cx));
    let t = ctx.tenant.slug.clone();
    let href_t = t.clone();
    let href_p = project.to_owned();
    let href = move |target: &solidate_app::core::LinkTarget| link_href(&href_t, &href_p, v, target);
    let app = app(cx);
    let r = app.render_doc(&ctx, project, &path, v, &href).await.or_http()?;
    let backlinks = app.backlinks(&ctx, project, &path).await.or_http()?;
    let sync = if r.view.inherited() {
        None
    } else {
        app.doc_sync(&ctx, project, &path).await.ok()
    };
    let stale = sync
        .as_ref()
        .map_or(0, |s| s.sections.iter().filter(|x| x.state.needs_attention()).count());
    let pending: std::collections::HashMap<String, SyncState> = sync
        .iter()
        .flat_map(|s| &s.sections)
        .filter(|x| x.state.needs_attention())
        .map(|x| (x.anchor.clone(), x.state))
        .collect();
    let (p, ps) = (project.to_owned(), path.as_str().to_owned());
    let title = r.view.document.title.clone().unwrap_or_else(|| ps.clone());
    let head = r.view.head.clone();

    Ok(view! {
        crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone(), path: Some(ps.clone()))
        <div class="head">
            <h1>(title)</h1>
            <div class="actions">
                <a class="button" href=(action_url(&t, &p, "edit", &ps, v))>
                    if r.view.inherited() { "Override" } else if head.is_some() { "Edit" } else { "Write" }
                </a>
                <a class="button secondary" href=(action_url(&t, &p, "history", &ps, v))>"History"</a>
                if sync.is_some() {
                    <a class="button secondary" href=(format!("{}/sync/{ps}", project_url(&t, &p)))>
                        "Sync " <span class=(if stale > 0 { "count warn" } else { "count" })>(stale)</span>
                    </a>
                }
            </div>
        </div>
        <div class="tabs">
            <a href=(doc_url(&t, &p, &ps, Variant::Human)) class=(if v == Variant::Human { "tab active" } else { "tab" })>"Human"</a>
            <a href=(doc_url(&t, &p, &ps, Variant::Ai)) class=(if v == Variant::Ai { "tab active" } else { "tab" })>"AI"</a>
        </div>
        if r.view.inherited() {
            <p class="notice">
                "Inherited from project " <a href=(doc_url(&t, &r.view.owner.slug, &ps, v))>(r.view.owner.slug.clone())</a>
                ". Editing creates an override in this project."
            </p>
        }
        <div class="doc-layout">
            <article class="markdown">
                match &head {
                    Some(_) => (Trusted(r.html.clone())),
                    None => {
                        <p class="muted">(format!("No {v} variant yet."))</p>
                    },
                }
            </article>
            <aside>
                if !r.outline.is_empty() {
                    <h2>"Contents"</h2>
                    <ul class="outline">
                        for o in &r.outline {
                            <li class=(format!("l{}", o.level))>
                                <a href=(format!("#{}", o.anchor))>(o.title.clone())</a>
                                match pending.get(&o.anchor) {
                                    Some(st) => <span class=(if *st == SyncState::Conflict { "dot conflict" } else { "dot" }) title=(st.as_str())></span>,
                                    None => "",
                                }
                            </li>
                        }
                    </ul>
                }
                if !backlinks.is_empty() {
                    <h2>"Linked from"</h2>
                    <ul class="plain">
                        for b in &backlinks {
                            <li><a href=(doc_url(&t, &b.project_slug, &b.path, b.variant))>(format!("{}/{}", b.project_slug, b.path))</a></li>
                        }
                    </ul>
                }
                match &head {
                    Some(h) => {
                        <h2>"Revision"</h2>
                        <p class="small muted">(fmt_time(h.updated_at))</p>
                        <p class="small"><code title=(h.content_hash.to_hex())>(h.content_hash.short())</code></p>
                    },
                    None => "",
                }
                if !r.dependencies.is_empty() {
                    <h2>"Includes"</h2>
                    <ul class="plain small">
                        for d in &r.dependencies {
                            <li class=(if d.hash.is_none() { "missing" } else { "" })>(d.target.to_string())</li>
                        }
                    </ul>
                }
                if !r.view.inherited() {
                    <form method="post" action=(action_url(&t, &p, "delete", &ps, Variant::Human)) class="danger"
                          onsubmit="return confirm('Delete this document (both variants)?')">
                        <button type="submit" class="link danger">"Delete document"</button>
                    </form>
                }
            </aside>
        </div>
    })
}

#[component]
async fn editor(
    action: String,
    cancel: String,
    content: String,
    base: String,
    #[default] path_field: bool,
    #[default] notice: Option<String>,
    #[default] resolves: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        match notice {
            Some(n) => <p class="alert">(n)</p>,
            None => "",
        }
        <form method="post" action=(action) class="editor">
            <input type="hidden" name="base" value=(base)>
            match resolves {
                Some(a) => <input type="hidden" name="resolves" value=(a)>,
                None => "",
            }
            if path_field {
                <label>"Path" <input type="text" name="path" required="" placeholder="design/overview" pattern="[A-Za-z0-9._/\\-]+"></label>
            }
            <div class="split">
                <textarea name="content" spellcheck="true"
                    hx-post="/preview" hx-trigger="input changed delay:400ms, load" hx-target="#preview">(content)</textarea>
                <div id="preview" class="markdown preview"></div>
            </div>
            <label>"Change note" <input type="text" name="message" maxlength="500" placeholder="Optional"></label>
            <div class="actions">
                <button type="submit">"Save"</button>
                <a class="button secondary" href=(cancel)>"Cancel"</a>
            </div>
        </form>
    })
}

#[page("/t/{tenant}/p/{project}/edit/{*path}")]
async fn edit(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path, v) = (path_param_segment(cx, "project"), doc_path(cx)?, variant(cx));
    let view_ = match app(cx).get_doc(&ctx, project, &path, v).await {
        Ok(d) => Some(d),
        Err(AppError::NotFound) => None,
        Err(e) => return Err(http(e)),
    };
    let head = view_.as_ref().and_then(|d| d.head.clone());
    let inherited = view_.as_ref().is_some_and(|d| d.inherited());
    let (t, p, ps) = (ctx.tenant.slug.clone(), project.to_owned(), path.as_str().to_owned());
    let content = match (&head, v) {
        (Some(h), _) => h.content.clone(),
        // Seed a new AI variant with the human text as a starting point.
        (None, Variant::Ai) => app(cx)
            .get_doc(&ctx, project, &path, Variant::Human)
            .await
            .ok()
            .and_then(|d| d.head)
            .map(|h| h.content)
            .unwrap_or_default(),
        (None, Variant::Human) => String::new(),
    };
    let base = head.map(|h| h.content_hash.to_hex()).unwrap_or_default();
    let resolves = resolves_param(cx);
    let reference = match &resolves {
        Some(a) => Some(app(cx).sync_item(&ctx, project, &path, a).await.or_http()?),
        None => None,
    };
    let other = match v {
        Variant::Human => Variant::Ai,
        Variant::Ai => Variant::Human,
    };
    Ok(view! {
        crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone(), path: Some(ps.clone()))
        <h1>(format!("Edit {ps} ({v})"))</h1>
        if inherited {
            <p class="notice">"Saving creates an override of the inherited document in this project."</p>
        }
        match &reference {
            Some(item) => {
                let (text, diff) = match other {
                    Variant::Human => (&item.human, &item.human_diff),
                    Variant::Ai => (&item.ai, &item.ai_diff),
                };
                <p class="notice">(format!("You are translating section #{} from the {other} variant. Saving marks it as in sync.", item.anchor))</p>
                <details open="">
                    <summary>(format!("{other} variant of #{}", item.anchor))</summary>
                    match diff.as_deref().filter(|d| !d.is_empty()) {
                        Some(d) => (diff_html(d)),
                        None => "",
                    }
                    match text {
                        Some(x) => <pre class="source">(x.body.clone())</pre>,
                        None => <p class="muted">"Section not present."</p>,
                    }
                </details>
            },
            None => "",
        }
        editor(
            action: action_url(&t, &p, "edit", &ps, v),
            cancel: doc_url(&t, &p, &ps, v),
            content: content,
            base: base,
            resolves: resolves,
        )
    })
}

#[derive(Deserialize)]
struct EditForm {
    content: String,
    base: String,
    message: Option<String>,
    path: Option<String>,
    resolves: Option<String>,
}

fn expect_from(base: &str) -> Result<Expect> {
    if base.is_empty() {
        return Ok(Expect::Absent);
    }
    base.parse::<Hash>()
        .map(Expect::Head)
        .map_err(|_| bad_request("invalid base").into())
}

#[page(POST "/t/{tenant}/p/{project}/edit/{*path}")]
async fn edit_submit(cx: &Cx, Form(form): Form<EditForm>) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path, v) = (path_param_segment(cx, "project"), doc_path(cx)?, variant(cx));
    let (t, p, ps) = (ctx.tenant.slug.clone(), project.to_owned(), path.as_str().to_owned());
    let content = form.content.replace("\r\n", "\n");
    let message = form.message.as_deref().map(str::trim).filter(|m| !m.is_empty());
    let resolves: Vec<String> = form.resolves.clone().filter(|a| !a.is_empty()).into_iter().collect();
    let result = app(cx)
        .put_doc(
            &ctx,
            PutDoc {
                project,
                path: &path,
                variant: v,
                content: &content,
                expect: expect_from(&form.base)?,
                message,
                resolves: &resolves,
            },
        )
        .await;
    match result {
        Ok(_) if !resolves.is_empty() => Err(see_other(format!("{}/sync/{ps}", project_url(&t, &p))).into()),
        Ok(_) => Err(see_other(doc_url(&t, &p, &ps, v)).into()),
        Err(AppError::PreconditionFailed { current }) => Ok(view! {
            (StatusCode::CONFLICT)
            crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone(), path: Some(ps.clone()))
            <h1>(format!("Edit {ps} ({v})"))</h1>
            editor(
                action: action_url(&t, &p, "edit", &ps, v),
                cancel: doc_url(&t, &p, &ps, v),
                content: content,
                base: current.map(|h| h.to_hex()).unwrap_or_default(),
                notice: Some("This document was changed by someone else while you were editing. Your text is kept below; review the current version, then save again to overwrite it.".to_owned()),
                resolves: resolves.into_iter().next(),
            )
        }),
        Err(e) => Err(http(e)),
    }
}

#[page("/t/{tenant}/p/{project}/new")]
async fn new_doc(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = app(cx)
        .project(&ctx, path_param_segment(cx, "project"))
        .await
        .or_http()?;
    let (t, p) = (ctx.tenant.slug.clone(), project.slug.clone());
    Ok(view! {
        crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone())
        <h1>"New document"</h1>
        editor(
            action: format!("{}/new", project_url(&t, &p)),
            cancel: project_url(&t, &p),
            content: "# Title\n\n".to_owned(),
            base: String::new(),
            path_field: true,
        )
    })
}

#[route(POST "/t/{tenant}/p/{project}/new")]
async fn new_doc_submit(cx: &Cx, Form(form): Form<EditForm>) -> Result<SeeOther> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let path = DocPath::parse(form.path.as_deref().unwrap_or("")).map_err(|e| bad_request(e.to_string()))?;
    let content = form.content.replace("\r\n", "\n");
    let r = app(cx)
        .put_doc(
            &ctx,
            PutDoc {
                project,
                path: &path,
                variant: Variant::Human,
                content: &content,
                expect: Expect::Absent,
                message: form.message.as_deref().filter(|m| !m.trim().is_empty()),
                resolves: &[],
            },
        )
        .await;
    match r {
        Ok(_) => Ok(see_other(doc_url(
            &ctx.tenant.slug,
            project,
            path.as_str(),
            Variant::Human,
        ))),
        Err(AppError::PreconditionFailed { .. } | AppError::AlreadyExists(_)) => {
            Err(bad_request(format!("a document already exists at {path}")).into())
        }
        Err(e) => Err(http(e)),
    }
}

#[route(POST "/t/{tenant}/p/{project}/delete/{*path}")]
async fn delete(cx: &Cx) -> Result<SeeOther> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    app(cx).delete_doc(&ctx, project, &path).await.or_http()?;
    Ok(see_other(project_url(&ctx.tenant.slug, project)))
}

#[page("/t/{tenant}/p/{project}/history/{*path}")]
async fn history(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path, v) = (path_param_segment(cx, "project"), doc_path(cx)?, variant(cx));
    let (doc, revs) = app(cx).history(&ctx, project, &path, v, 100).await.or_http()?;
    let (t, p, ps) = (ctx.tenant.slug.clone(), project.to_owned(), path.as_str().to_owned());
    let owner = doc.owner.slug.clone();
    Ok(view! {
        crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone(), path: Some(ps.clone()))
        <h1>(format!("History of {ps} ({v})"))</h1>
        if doc.inherited() {
            <p class="notice">(format!("Showing history of the inherited document in {owner}."))</p>
        }
        <table class="docs">
            <thead><tr><th>"When"</th><th>"Revision"</th><th>"Author"</th><th>"Note"</th></tr></thead>
            <tbody>
                for r in &revs {
                    let author = match (r.author_user_id, r.author_token_id) {
                        (Some(_), _) => "user",
                        (_, Some(_)) => "api token",
                        _ => "system",
                    };
                    <tr>
                        <td class="small">(fmt_time(r.created_at))</td>
                        <td><a href=(format!("/t/{t}/p/{owner}/rev/{}/{ps}", r.id))><code>(r.content_hash.short())</code></a></td>
                        <td class="small">(author)</td>
                        <td>(r.message.clone().unwrap_or_default())</td>
                    </tr>
                }
            </tbody>
        </table>
    })
}

#[page("/t/{tenant}/p/{project}/rev/{rev}/{*path}")]
async fn revision(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let rev = path_param_segment(cx, "rev").parse().map_err(|_| not_found())?;
    let d = app(cx).revision_diff(&ctx, project, &path, rev).await.or_http()?;
    let (t, p, ps) = (ctx.tenant.slug.clone(), project.to_owned(), path.as_str().to_owned());
    let v = d.revision.variant;
    Ok(view! {
        crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone(), path: Some(ps.clone()))
        <h1>(format!("Revision {} ({v})", d.revision.content_hash.short()))</h1>
        <p class="muted">(fmt_time(d.revision.created_at)) " · " (d.revision.message.clone().unwrap_or_default())</p>
        <p><a href=(action_url(&t, &p, "history", &ps, v))>"Back to history"</a></p>
        if d.diff.is_empty() {
            <p class="muted">"No changes."</p>
        } else {
            (diff_html(&d.diff))
        }
        <details>
            <summary>"Full text"</summary>
            <pre class="source">(d.content.clone())</pre>
        </details>
    })
}

#[derive(Deserialize)]
struct PreviewForm {
    content: String,
}

/// Renders Markdown for the editor preview (htmx). Links are left unresolved.
#[route(POST "/preview")]
async fn preview(cx: &Cx, Form(form): Form<PreviewForm>) -> Result<Html<String>> {
    require_user(cx).await?;
    if form.content.len() > app(cx).config().max_doc_bytes {
        return Err(bad_request("document too large").into());
    }
    Ok(Html(render_html(&form.content, None, &|_| None).html))
}
