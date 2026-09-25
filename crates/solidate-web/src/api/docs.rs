//! Document reads and writes, history, revisions, and backlinks.

use serde::{Deserialize, Serialize};
use solidate_app::core::{Hash, Variant, analyze};
use solidate_app::db::Expect;
use solidate_app::{AppError, PutDoc};
use time::OffsetDateTime;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::request::content_type;
use topcoat::router::response::Response;
use topcoat::router::{Body, StatusCode, path_param_segment, query_params, route};

use super::{
    ApiError, ApiResult, api_ctx, doc_path, finish, json, not_modified, parse_variant, text, wants_markdown, with_etag,
    write_precondition,
};
use crate::auth::app;

#[query_params]
struct DocQuery {
    variant: Option<String>,
    /// Return only this section (anchor).
    section: Option<String>,
    /// `1`: expand `{{include}}` directives. The ETag becomes the resolved hash.
    expand: Option<String>,
    /// `md` for raw Markdown, `json` for JSON. Default: from `Accept`.
    format: Option<String>,
    /// PUT with a Markdown body: change note.
    message: Option<String>,
    /// PUT with a Markdown body: comma-separated anchors to mark in sync.
    resolves: Option<String>,
}

fn doc_query(cx: &Cx) -> Result<&DocQuery, ApiError> {
    query_params::<DocQuery>(cx).map_err(|e| ApiError::bad_request(e.to_string()))
}

#[derive(Serialize)]
struct SectionOut<'a> {
    anchor: &'a str,
    title: &'a str,
    level: u8,
    parent: Option<&'a str>,
    /// Semantic hash; the value sync compares.
    hash: Hash,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
}

#[derive(Serialize)]
struct DependencyOut {
    target: String,
    /// `None` when the include could not be resolved.
    hash: Option<Hash>,
}

#[derive(Serialize)]
struct DocOut<'a> {
    project: &'a str,
    /// Project that owns the document; differs from `project` when inherited.
    owner: &'a str,
    path: &'a str,
    variant: Variant,
    title: Option<&'a str>,
    inherited: bool,
    sync_enabled: bool,
    /// Pass as `If-Match` when writing this variant.
    content_hash: Hash,
    /// Content hash combined with included content; present with `expand=1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_hash: Option<Hash>,
    updated_at: OffsetDateTime,
    sections: Vec<SectionOut<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<DependencyOut>,
    content: &'a str,
}

#[route(GET "/api/v1/projects/{project}/docs/{*path}")]
async fn api_get(cx: &Cx) -> Result<Response> {
    finish(get_inner(cx).await)
}

async fn get_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path, q) = (path_param_segment(cx, "project"), doc_path(cx)?, doc_query(cx)?);
    let variant = parse_variant(q.variant.as_deref())?;
    let expand = q.expand.as_deref() == Some("1");
    let e = app(cx).expanded_doc(&ctx, project, &path, variant).await?;
    let head = e.view.head.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "variant_missing",
            format!("the {variant} variant has not been written"),
        )
    })?;
    let (content, tag) = if expand {
        (e.text.as_deref().unwrap_or_default(), e.resolved_hash)
    } else {
        (head.content.as_str(), Some(head.content_hash))
    };
    if let Some(r) = not_modified(cx, tag)? {
        return Ok(r);
    }
    let analysis = analyze(content, Some(&path));
    let markdown = wants_markdown(cx, q.format.as_deref());

    if let Some(anchor) = q.section.as_deref() {
        let s = analysis.section(anchor).ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "section_missing",
                format!("no section #{anchor}"),
            )
        })?;
        let res = if markdown {
            text("text/markdown; charset=utf-8", s.body.clone())
        } else {
            json(
                StatusCode::OK,
                &SectionOut {
                    anchor: &s.anchor,
                    title: &s.title,
                    level: s.level,
                    parent: s.parent.as_deref(),
                    hash: s.hash,
                    body: Some(&s.body),
                },
            )
        };
        return Ok(with_etag(res, tag));
    }
    if markdown {
        return Ok(with_etag(text("text/markdown; charset=utf-8", content.to_owned()), tag));
    }
    let out = DocOut {
        project: &e.view.project.slug,
        owner: &e.view.owner.slug,
        path: &e.view.document.path,
        variant,
        title: head.title.as_deref(),
        inherited: e.view.inherited(),
        sync_enabled: e.view.document.sync_enabled,
        content_hash: head.content_hash,
        resolved_hash: expand.then_some(e.resolved_hash).flatten(),
        updated_at: head.updated_at,
        sections: analysis
            .sections
            .iter()
            .map(|s| SectionOut {
                anchor: &s.anchor,
                title: &s.title,
                level: s.level,
                parent: s.parent.as_deref(),
                hash: s.hash,
                body: None,
            })
            .collect(),
        dependencies: if expand {
            e.dependencies
                .iter()
                .map(|d| DependencyOut {
                    target: d.target.to_string(),
                    hash: d.hash,
                })
                .collect()
        } else {
            Vec::new()
        },
        content,
    };
    Ok(with_etag(json(StatusCode::OK, &out), tag))
}

#[derive(Deserialize)]
struct PutBody {
    content: String,
    message: Option<String>,
    #[serde(default)]
    resolves: Vec<String>,
}

#[derive(Serialize)]
struct SyncOut<'a> {
    anchor: &'a str,
    state: solidate_app::core::SyncState,
}

/// Writes one variant. Body: `text/markdown` (note and anchors via `message` and
/// `resolves` query parameters) or JSON `{content, message?, resolves?}`.
/// 201 when the variant was created (`If-None-Match: *`), 200 otherwise.
#[route(PUT "/api/v1/projects/{project}/docs/{*path}")]
async fn api_put(cx: &Cx, body: String) -> Result<Response> {
    finish(put_inner(cx, body).await)
}

async fn put_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path, q) = (path_param_segment(cx, "project"), doc_path(cx)?, doc_query(cx)?);
    let variant = parse_variant(q.variant.as_deref())?;
    let expect = write_precondition(cx)?;
    let is_json = content_type(cx).is_some_and(|c| c.starts_with("application/json"));
    let req = if is_json {
        serde_json::from_str::<PutBody>(&body).map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))?
    } else {
        PutBody {
            content: body,
            message: q.message.clone(),
            resolves: q
                .resolves
                .as_deref()
                .map(|r| {
                    r.split(',')
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        }
    };
    let content = req.content.replace("\r\n", "\n");
    let r = app(cx)
        .put_doc(
            &ctx,
            PutDoc {
                project,
                path: &path,
                variant,
                content: &content,
                expect,
                message: req.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                resolves: &req.resolves,
            },
        )
        .await
        .map_err(|e| match e {
            // Creating over an existing document.
            AppError::AlreadyExists(_) if expect == Expect::Absent => {
                ApiError::from(AppError::PreconditionFailed { current: None })
            }
            e => e.into(),
        })?;
    #[derive(Serialize)]
    struct Out<'a> {
        path: &'a str,
        variant: Variant,
        content_hash: Hash,
        revision: String,
        /// `false` when the content equalled the current head.
        changed: bool,
        /// Sections of the document needing sync after this write.
        sync: Vec<SyncOut<'a>>,
    }
    let status = if expect == Expect::Absent {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let res = json(
        status,
        &Out {
            path: &r.document.path,
            variant,
            content_hash: r.revision.content_hash,
            revision: r.revision.id.to_string(),
            changed: r.created,
            sync: r
                .sync
                .iter()
                .filter(|s| s.state.needs_attention())
                .map(|s| SyncOut {
                    anchor: &s.anchor,
                    state: s.state,
                })
                .collect(),
        },
    );
    Ok(with_etag(res, Some(r.revision.content_hash)))
}

/// Deletes both variants of a document owned by the project.
#[route(DELETE "/api/v1/projects/{project}/docs/{*path}")]
async fn api_delete(cx: &Cx) -> Result<Response> {
    finish(delete_inner(cx).await)
}

async fn delete_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    app(cx).delete_doc(&ctx, project, &path).await?;
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::NO_CONTENT;
    Ok(res)
}

#[query_params]
struct HistoryQuery {
    variant: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct RevisionOut {
    id: String,
    variant: Variant,
    content_hash: Hash,
    /// `user`, `token`, or `system`.
    author: &'static str,
    message: Option<String>,
    created_at: OffsetDateTime,
}

fn revision_out(r: &solidate_app::db::Revision) -> RevisionOut {
    RevisionOut {
        id: r.id.to_string(),
        variant: r.variant,
        content_hash: r.content_hash,
        author: match (r.author_user_id, r.author_token_id) {
            (Some(_), _) => "user",
            (_, Some(_)) => "token",
            _ => "system",
        },
        message: r.message.clone(),
        created_at: r.created_at,
    }
}

/// Revisions of one variant, newest first.
#[route(GET "/api/v1/projects/{project}/history/{*path}")]
async fn api_history(cx: &Cx) -> Result<Response> {
    finish(history_inner(cx).await)
}

async fn history_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let q = query_params::<HistoryQuery>(cx).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let variant = parse_variant(q.variant.as_deref())?;
    let (_, revs) = app(cx)
        .history(&ctx, project, &path, variant, q.limit.unwrap_or(50))
        .await?;
    Ok(json(StatusCode::OK, &revs.iter().map(revision_out).collect::<Vec<_>>()))
}

/// One revision's content and its diff from the parent revision.
#[route(GET "/api/v1/projects/{project}/revisions/{rev}/{*path}")]
async fn api_revision(cx: &Cx) -> Result<Response> {
    finish(revision_inner(cx).await)
}

async fn revision_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let rev = path_param_segment(cx, "rev")
        .parse()
        .map_err(|_| ApiError::from(AppError::NotFound))?;
    let d = app(cx).revision_diff(&ctx, project, &path, rev).await?;
    #[derive(Serialize)]
    struct Out<'a> {
        revision: RevisionOut,
        content: &'a str,
        diff: &'a str,
    }
    Ok(json(
        StatusCode::OK,
        &Out {
            revision: revision_out(&d.revision),
            content: &d.content,
            diff: &d.diff,
        },
    ))
}

#[route(GET "/api/v1/projects/{project}/backlinks/{*path}")]
async fn api_backlinks(cx: &Cx) -> Result<Response> {
    finish(backlinks_inner(cx).await)
}

async fn backlinks_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let links = app(cx).backlinks(&ctx, project, &path).await?;
    #[derive(Serialize)]
    struct Out {
        project: String,
        path: String,
        variant: Variant,
        anchor: Option<String>,
    }
    let out: Vec<_> = links
        .into_iter()
        .map(|l| Out {
            project: l.project_slug,
            path: l.path,
            variant: l.variant,
            anchor: l.target_anchor,
        })
        .collect();
    Ok(json(StatusCode::OK, &out))
}
