//! Document reads and writes, history, revisions, restores, undeletes, and backlinks.

use serde::{Deserialize, Serialize};
use solidate_app::core::edit::span_hash;
use solidate_app::core::{Hash, SectionTarget, Variant, analyze};
use solidate_app::db::Expect;
use solidate_app::{AppError, PutDoc, PutSection, Restore};
use time::OffsetDateTime;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::request::content_type;
use topcoat::router::response::Response;
use topcoat::router::{Body, StatusCode, path_param_segment, query_params, route};

use super::{
    ApiError, ApiResult, api_ctx, doc_path, finish, json, not_modified, optional_write_precondition, parse_variant,
    text, wants_markdown, with_etag, write_precondition,
};
use crate::auth::app;

#[query_params]
struct DocQuery {
    variant: Option<String>,
    /// Return only this section (anchor).
    section: Option<String>,
    /// `1` with `section`: include its subsections.
    subsections: Option<String>,
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
    #[serde(with = "time::serde::rfc3339")]
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
        let subsections = q.subsections.as_deref() == Some("1");
        let body = match subsections {
            true => analysis.section_tree_source(anchor).unwrap_or_default(),
            false => s.body.clone(),
        };
        let hash = span_hash(&analysis, SectionTarget::Section { anchor, subsections }).unwrap_or(s.hash);
        let res = if markdown {
            text("text/markdown; charset=utf-8", body)
        } else {
            json(
                StatusCode::OK,
                &SectionOut {
                    anchor: &s.anchor,
                    title: &s.title,
                    level: s.level,
                    parent: s.parent.as_deref(),
                    hash,
                    body: Some(&body),
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
        /// New AI content hash when this write's diagram edits were carried over.
        #[serde(skip_serializing_if = "Option::is_none")]
        followed_ai_hash: Option<Hash>,
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
            followed_ai_hash: r.followed,
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

#[derive(Deserialize)]
struct PatchBody {
    /// Section to replace; empty `content` deletes it.
    anchor: Option<String>,
    /// Instead of `anchor`: insert after this section and its subsections. Neither
    /// appends at the end.
    after: Option<String>,
    #[serde(default)]
    subsections: bool,
    content: String,
    /// The target section's `hash` as last read.
    section_hash: Option<Hash>,
    message: Option<String>,
    #[serde(default)]
    resolves: Vec<String>,
}

/// Replaces, deletes or inserts one section of a variant. JSON body
/// `{anchor? | after?, subsections?, content, section_hash?, message?, resolves?}`.
/// Replacing requires `section_hash` or `If-Match`; insertions accept either but
/// need neither.
#[route(PATCH "/api/v1/projects/{project}/docs/{*path}")]
async fn api_patch(cx: &Cx, body: String) -> Result<Response> {
    finish(patch_inner(cx, body).await)
}

async fn patch_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path, q) = (path_param_segment(cx, "project"), doc_path(cx)?, doc_query(cx)?);
    let variant = parse_variant(q.variant.as_deref())?;
    let req = serde_json::from_str::<PatchBody>(&body)
        .map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))?;
    let target = match (req.anchor.as_deref(), req.after.as_deref()) {
        (Some(anchor), None) => SectionTarget::Section {
            anchor,
            subsections: req.subsections,
        },
        (None, Some(after)) => SectionTarget::After(after),
        (None, None) => SectionTarget::End,
        (Some(_), Some(_)) => return Err(ApiError::bad_request("use either anchor or after")),
    };
    let precondition = optional_write_precondition(cx)?;
    if matches!(target, SectionTarget::Section { .. }) && precondition.is_none() && req.section_hash.is_none() {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "precondition_required",
            "replacing a section requires section_hash or If-Match",
        ));
    }
    let content = req.content.replace("\r\n", "\n");
    let r = app(cx)
        .put_section(
            &ctx,
            PutSection {
                project,
                path: &path,
                variant,
                target,
                content: &content,
                expect: precondition.unwrap_or(Expect::Any),
                section_hash: req.section_hash,
                message: req.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                resolves: &req.resolves,
            },
        )
        .await?;
    #[derive(Serialize)]
    struct Out<'a> {
        path: &'a str,
        variant: Variant,
        content_hash: Hash,
        revision: String,
        changed: bool,
        /// Anchors of the sections written.
        anchors: &'a [String],
        sync: Vec<SyncOut<'a>>,
        /// New AI content hash when this write's diagram edits were carried over.
        #[serde(skip_serializing_if = "Option::is_none")]
        followed_ai_hash: Option<Hash>,
    }
    let res = json(
        StatusCode::OK,
        &Out {
            path: &r.put.document.path,
            variant,
            content_hash: r.put.revision.content_hash,
            revision: r.put.revision.id.to_string(),
            changed: r.put.created,
            anchors: &r.anchors,
            followed_ai_hash: r.put.followed,
            sync: r
                .put
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
    Ok(with_etag(res, Some(r.put.revision.content_hash)))
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

#[derive(Serialize)]
struct DeletedOut<'a> {
    /// Document id; pass to the restore route.
    id: String,
    path: &'a str,
    title: Option<&'a str>,
    #[serde(with = "time::serde::rfc3339")]
    deleted_at: OffsetDateTime,
    /// `user`, `token`, or `system`.
    deleted_by: &'a str,
    /// User or token name.
    deleted_by_name: Option<&'a str>,
    /// Content hash of each variant's last head.
    human_hash: Option<Hash>,
    ai_hash: Option<Hash>,
}

fn deleted_out(d: &solidate_app::db::DeletedDocument) -> DeletedOut<'_> {
    DeletedOut {
        id: d.id.to_string(),
        path: &d.path,
        title: d.title.as_deref(),
        deleted_at: d.deleted_at,
        deleted_by: &d.deleted_by_kind,
        deleted_by_name: d.deleted_by_name.as_deref(),
        human_hash: d.human_hash,
        ai_hash: d.ai_hash,
    }
}

fn document_id(cx: &Cx) -> Result<solidate_app::db::DocumentId, ApiError> {
    path_param_segment(cx, "id")
        .parse()
        .map_err(|_| ApiError::from(AppError::NotFound))
}

/// Deleted documents owned by the project, newest deletion first.
#[route(GET "/api/v1/projects/{project}/deleted")]
async fn api_deleted(cx: &Cx) -> Result<Response> {
    finish(deleted_inner(cx).await)
}

async fn deleted_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let docs = app(cx).deleted_docs(&ctx, path_param_segment(cx, "project")).await?;
    Ok(json(StatusCode::OK, &docs.iter().map(deleted_out).collect::<Vec<_>>()))
}

/// One deleted document with the last content of each variant.
#[route(GET "/api/v1/projects/{project}/deleted/{id}")]
async fn api_deleted_doc(cx: &Cx) -> Result<Response> {
    finish(deleted_doc_inner(cx).await)
}

async fn deleted_doc_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let v = app(cx)
        .deleted_doc(&ctx, path_param_segment(cx, "project"), document_id(cx)?)
        .await?;
    #[derive(Serialize)]
    struct Out<'a> {
        #[serde(flatten)]
        document: DeletedOut<'a>,
        human: Option<&'a str>,
        ai: Option<&'a str>,
    }
    Ok(json(
        StatusCode::OK,
        &Out {
            document: deleted_out(&v.document),
            human: v.human.as_ref().map(|h| h.content.as_str()),
            ai: v.ai.as_ref().map(|h| h.content.as_str()),
        },
    ))
}

/// Makes a deleted document live again at its path. `409 already_exists` when a
/// live document holds the path.
#[route(POST "/api/v1/projects/{project}/deleted/{id}/restore")]
async fn api_undelete(cx: &Cx) -> Result<Response> {
    finish(undelete_inner(cx).await)
}

async fn undelete_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let d = app(cx)
        .undelete_doc(&ctx, path_param_segment(cx, "project"), document_id(cx)?)
        .await?;
    #[derive(Serialize)]
    struct Out<'a> {
        id: String,
        path: &'a str,
        title: Option<&'a str>,
    }
    Ok(json(
        StatusCode::OK,
        &Out {
            id: d.id.to_string(),
            path: &d.path,
            title: d.title.as_deref(),
        },
    ))
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
    /// The revision whose content this one restored.
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_from: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
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
        restored_from: r.restored_from.map(|id| id.to_string()),
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

#[derive(Deserialize)]
struct RestoreBody {
    revision: String,
    /// Also restore the other variant's paired sections.
    #[serde(default = "yes")]
    companion: bool,
    message: Option<String>,
    /// Return the result without writing it.
    #[serde(default)]
    dry_run: bool,
}

fn yes() -> bool {
    true
}

/// Writes an earlier revision's content back as the head of its variant. JSON body
/// `{revision, companion?, message?, dry_run?}`; `If-Match` applies to the head of
/// the revision's variant and is required unless `dry_run`.
#[route(POST "/api/v1/projects/{project}/restore/{*path}")]
async fn api_restore(cx: &Cx, body: String) -> Result<Response> {
    finish(restore_inner(cx, body).await)
}

async fn restore_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let req = serde_json::from_str::<RestoreBody>(&body)
        .map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))?;
    let revision = req
        .revision
        .parse()
        .map_err(|_| ApiError::bad_request("revision must be a revision id"))?;
    let expect = match req.dry_run {
        true => optional_write_precondition(cx)?.unwrap_or(Expect::Any),
        false => write_precondition(cx)?,
    };
    let r = app(cx)
        .restore(
            &ctx,
            Restore {
                project,
                path: &path,
                revision,
                expect,
                message: req.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                companion: req.companion,
                dry_run: req.dry_run,
            },
        )
        .await?;
    #[derive(Serialize)]
    struct CompanionOut<'a> {
        variant: Variant,
        revision: String,
        content_hash: Hash,
        anchors: &'a [String],
        diff: &'a str,
    }
    #[derive(Serialize)]
    struct Out<'a> {
        path: &'a str,
        variant: Variant,
        restored_from: String,
        revision: String,
        content_hash: Hash,
        changed: bool,
        dry_run: bool,
        diff: &'a str,
        companion: Option<CompanionOut<'a>>,
        skipped: &'a [String],
        reconciled: &'a [String],
        sync: Vec<SyncOut<'a>>,
    }
    let res = json(
        StatusCode::OK,
        &Out {
            path: &r.put.document.path,
            variant: r.put.revision.variant,
            restored_from: r.restored_from.id.to_string(),
            revision: r.put.revision.id.to_string(),
            content_hash: r.put.revision.content_hash,
            changed: r.put.created,
            dry_run: r.dry_run,
            diff: &r.diff,
            companion: r.companion.as_ref().map(|c| CompanionOut {
                variant: c.revision.variant,
                revision: c.revision.id.to_string(),
                content_hash: c.revision.content_hash,
                anchors: &c.anchors,
                diff: &c.diff,
            }),
            skipped: &r.skipped,
            reconciled: &r.reconciled,
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
    Ok(with_etag(res, (!r.dry_run).then_some(r.put.revision.content_hash)))
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
