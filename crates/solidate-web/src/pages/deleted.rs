//! Deleted documents: list, read-only view, and undelete.

use solidate_app::AppError;
use solidate_app::core::{Variant, render_html};
use solidate_app::db::{DeletedDocument, DocumentId};
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::error::{not_found, see_other};
use topcoat::router::{StatusCode, page, path_param_segment, query_params};
use topcoat::view::{View, component, view};

use crate::auth::{app, tenant_ctx};
use crate::error::{OrHttp, http};
use crate::ui::{Trusted, doc_url, fmt_time, parse_variant, project_url, tenant_url};

fn deleted_url(t: &str, p: &str) -> String {
    format!("{}/deleted", project_url(t, p))
}

pub fn deleted_doc_url(t: &str, p: &str, id: DocumentId) -> String {
    format!("{}/{id}", deleted_url(t, p))
}

fn document_id(cx: &Cx) -> Result<DocumentId> {
    Ok(path_param_segment(cx, "id").parse().map_err(|_| not_found())?)
}

fn deleted_by(d: &DeletedDocument) -> String {
    match (d.deleted_by_kind.as_str(), d.deleted_by_name.as_deref()) {
        ("token", Some(n)) => format!("token {n}"),
        (_, Some(n)) => n.to_owned(),
        ("system", None) => "system".to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// Form posting an undelete of document `id`.
#[component]
pub async fn undelete_button(
    tenant: String,
    project: String,
    id: DocumentId,
    label: &'static str,
) -> Result<impl View> {
    Ok(view! {
        <form method="post" action=(format!("{}/restore", deleted_doc_url(&tenant, &project, id))) class="inline">
            <button type="submit" class="link">(label)</button>
        </form>
    })
}

#[component]
async fn crumbs(
    tenant: String,
    tenant_name: String,
    project: String,
    #[default] last: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&tenant))>(tenant_name)</a>
            <a href=(project_url(&tenant, &project))>(project.clone())</a>
            match last {
                Some(l) => {
                    <a href=(deleted_url(&tenant, &project))>"Deleted"</a>
                    <span>(l)</span>
                },
                None => <span>"Deleted"</span>,
            }
        </nav>
    })
}

#[page("/t/{tenant}/p/{project}/deleted")]
async fn deleted_list(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let docs = app(cx).deleted_docs(&ctx, project).await.or_http()?;
    let (t, p) = (ctx.tenant.slug.clone(), project.to_owned());
    Ok(view! {
        crumbs(tenant: t.clone(), tenant_name: ctx.tenant.name.clone(), project: p.clone())
        <h1>"Deleted documents"</h1>
        <p class="muted">"Deleted documents keep their history. Restoring one brings back both variants and their sync state."</p>
        if docs.is_empty() {
            <p class="muted">"No deleted documents."</p>
        } else {
            <table class="docs">
                <thead><tr><th>"Path"</th><th>"Title"</th><th>"Deleted"</th><th>"By"</th><th></th></tr></thead>
                <tbody>
                    for d in &docs {
                        <tr>
                            <td><a href=(deleted_doc_url(&t, &p, d.id))><code>(d.path.clone())</code></a></td>
                            <td>(d.title.clone().unwrap_or_default())</td>
                            <td class="muted small">(fmt_time(d.deleted_at))</td>
                            <td class="small">(deleted_by(d))</td>
                            <td>undelete_button(tenant: t.clone(), project: p.clone(), id: d.id, label: "Restore")</td>
                        </tr>
                    }
                </tbody>
            </table>
        }
    })
}

#[query_params]
struct DeletedQuery {
    v: Option<String>,
}

#[component]
async fn deleted_view(
    tenant: String,
    tenant_name: String,
    project: String,
    view_: solidate_app::DeletedDocView,
    v: Variant,
    #[default] notice: Option<Trusted>,
) -> Result<impl View> {
    let d = &view_.document;
    let path = d.path.clone();
    let title = d.title.clone().unwrap_or_else(|| path.clone());
    let info = format!("Deleted {} by {}. Read-only.", fmt_time(d.deleted_at), deleted_by(d));
    let head = match v {
        Variant::Human => view_.human.as_ref(),
        Variant::Ai => view_.ai.as_ref(),
    };
    let html = head.map(|h| render_html(&h.content, None, &|_| None).html);
    let here = deleted_doc_url(&tenant, &project, d.id);
    Ok(view! {
        crumbs(tenant: tenant.clone(), tenant_name: tenant_name, project: project.clone(), last: Some(path))
        <div class="head">
            <h1>(title)</h1>
            <div class="actions">
                <form method="post" action=(format!("{here}/restore")) class="inline">
                    <button type="submit">"Restore document"</button>
                </form>
            </div>
        </div>
        match notice {
            Some(n) => (n),
            None => "",
        }
        <p class="notice">(info)</p>
        <div class="tabs">
            <a href=(format!("{here}?v=human")) class=(if v == Variant::Human { "tab active" } else { "tab" })>"Human"</a>
            <a href=(format!("{here}?v=ai")) class=(if v == Variant::Ai { "tab active" } else { "tab" })>"AI"</a>
        </div>
        <article class="markdown">
            match html {
                Some(h) => (Trusted(h)),
                None => {
                    <p class="muted">(format!("No {v} variant."))</p>
                },
            }
        </article>
    })
}

#[page("/t/{tenant}/p/{project}/deleted/{id}")]
async fn deleted_doc(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let v = parse_variant(query_params::<DeletedQuery>(cx).ok().and_then(|q| q.v.as_deref()));
    let view_ = app(cx).deleted_doc(&ctx, project, document_id(cx)?).await.or_http()?;
    Ok(view! {
        deleted_view(
            tenant: ctx.tenant.slug.clone(),
            tenant_name: ctx.tenant.name.clone(),
            project: project.to_owned(),
            view_: view_,
            v: v,
        )
    })
}

/// Undeletes and redirects to the document; `409` with the document's page when a
/// live document holds its path.
#[page(POST "/t/{tenant}/p/{project}/deleted/{id}/restore")]
async fn undelete(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let project = path_param_segment(cx, "project");
    let id = document_id(cx)?;
    let (t, p) = (ctx.tenant.slug.clone(), project.to_owned());
    match app(cx).undelete_doc(&ctx, project, id).await {
        Ok(d) => {
            let to = doc_url(&t, &p, &d.path, Variant::Human);
            let sep = if to.contains('?') { '&' } else { '?' };
            Err(see_other(format!("{to}{sep}undeleted=1")).into())
        }
        Err(AppError::AlreadyExists(_)) => {
            let view_ = app(cx).deleted_doc(&ctx, project, id).await.or_http()?;
            let live = doc_url(&t, &p, &view_.document.path, Variant::Human);
            let notice = Trusted(format!(
                "<p class=\"notice\">A document exists at <a href=\"{}\"><code>{}</code></a>. Delete it before \
                 restoring this one.</p>",
                crate::ui::escape(&live),
                crate::ui::escape(&view_.document.path)
            ));
            Ok(view! {
                (StatusCode::CONFLICT)
                deleted_view(
                    tenant: t,
                    tenant_name: ctx.tenant.name.clone(),
                    project: p,
                    view_: view_,
                    v: Variant::Human,
                    notice: Some(notice),
                )
            })
        }
        Err(e) => Err(http(e)),
    }
}
