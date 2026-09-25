//! Project home and search.

use solidate_app::core::Variant;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::context::app_context;
use topcoat::router::{page, path_param_segment, query_params, route};
use topcoat::view::{View, view};

use crate::WebConfig;
use crate::auth::{app, tenant_ctx};
use crate::error::OrHttp;
use crate::ui::{Trusted, doc_url, fmt_time, project_url, tenant_url};

#[page("/t/{tenant}/p/{project}")]
async fn project_home(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let app = app(cx);
    let tree = app.tree(&ctx, path_param_segment(cx, "project")).await.or_http()?;
    let chain = app.project_chain(&ctx, &tree.project.slug).await.or_http()?;
    let queue = app.sync_queue(&ctx, &tree.project.slug).await.or_http()?;
    let (t, p) = (ctx.tenant.slug.clone(), tree.project.slug.clone());
    let ancestors: Vec<String> = chain.iter().skip(1).map(|x| x.slug.clone()).collect();

    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&t))>(ctx.tenant.name.as_str())</a>
            <span>(tree.project.name.as_str())</span>
        </nav>
        <div class="head">
            <h1>(tree.project.name.as_str())</h1>
            <div class="actions">
                <a class="button" href=(format!("{}/new", project_url(&t, &p)))>"New document"</a>
                <a class="button secondary" href=(format!("{}/sync", project_url(&t, &p)))>
                    "Sync queue " <span class="count">(queue.len())</span>
                </a>
                <a class="button secondary" href=(format!("{}/llms.txt", project_url(&t, &p)))>"llms.txt"</a>
            </div>
        </div>
        if !ancestors.is_empty() {
            <p class="muted">"Inherits from " (ancestors.join(" → "))</p>
        }
        <form method="get" action=(format!("/t/{t}/search")) class="search">
            <input type="hidden" name="project" value=(p.clone())>
            <input type="search" name="q" placeholder="Search this project">
        </form>
        <p class="muted small">"Root hash " <code title=(tree.root_hash.to_hex())>(tree.root_hash.short())</code></p>
        if tree.entries.is_empty() {
            <p class="muted">"No documents yet."</p>
        } else {
            <table class="docs">
                <thead><tr><th>"Path"</th><th>"Title"</th><th>"Variants"</th><th>"Updated"</th></tr></thead>
                <tbody>
                    for e in &tree.entries {
                        <tr>
                            <td>
                                <a href=(doc_url(&t, &p, &e.path, Variant::Human))><code>(e.path.as_str())</code></a>
                                if e.inherited {
                                    <span class="badge" title="Inherited">(format!("from {}", e.owner))</span>
                                }
                            </td>
                            <td>(e.title.clone().unwrap_or_default())</td>
                            <td>
                                if e.human.is_some() { <span class="badge">"human"</span> }
                                if e.ai.is_some() { <span class="badge ai">"ai"</span> }
                                if !e.sync_enabled { <span class="badge muted">"sync off"</span> }
                            </td>
                            <td class="muted small">(fmt_time(e.updated_at))</td>
                        </tr>
                    }
                </tbody>
            </table>
        }
    })
}

#[query_params]
struct SearchQuery {
    q: Option<String>,
    project: Option<String>,
}

#[page("/t/{tenant}/search")]
async fn search(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let query = query_params::<SearchQuery>(cx).ok();
    let q = query.and_then(|x| x.q.clone()).unwrap_or_default();
    let project = query.and_then(|x| x.project.clone()).filter(|s| !s.is_empty());
    let hits = if q.trim().is_empty() {
        Vec::new()
    } else {
        app(cx).search(&ctx, &q, project.as_deref(), 50).await.or_http()?
    };
    let t = ctx.tenant.slug.clone();
    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&t))>(ctx.tenant.name.as_str())</a>
            <span>"Search"</span>
        </nav>
        <form method="get" class="search">
            <input type="search" name="q" value=(q.clone()) autofocus="">
            <input type="hidden" name="project" value=(project.clone().unwrap_or_default())>
            <button type="submit">"Search"</button>
        </form>
        if !q.trim().is_empty() {
            <p class="muted">(format!("{} results", hits.len()))</p>
        }
        <ol class="results">
            for h in &hits {
                <li>
                    <a href=(doc_url(&t, &h.project_slug, &h.path, h.variant))>
                        (h.title.clone().unwrap_or_else(|| h.path.clone()))
                    </a>
                    <span class="muted small">(format!(" {}/{} · {}", h.project_slug, h.path, h.variant))</span>
                    <p class="snippet">(Trusted(h.snippet_html()))</p>
                </li>
            }
        </ol>
    })
}

/// `llms.txt` for signed-in browser sessions; the same index as the API route.
#[route(GET "/t/{tenant}/p/{project}/llms.txt")]
async fn llms_txt(cx: &Cx) -> Result<(topcoat::router::HeaderMap, String)> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let base = app_context::<WebConfig>(cx).public_url.as_deref().unwrap_or("");
    let body = crate::api::llms_txt(app(cx), &ctx, path_param_segment(cx, "project"), base)
        .await
        .map_err(|_| topcoat::router::error::not_found())?;
    let mut headers = topcoat::router::HeaderMap::new();
    headers.insert(
        topcoat::router::header::CONTENT_TYPE,
        topcoat::router::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok((headers, body))
}
