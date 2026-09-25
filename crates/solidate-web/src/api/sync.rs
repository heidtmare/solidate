//! Sync queue, per-document status and items, resolution, translation guide and
//! translation proposals.

use serde::Deserialize;
use solidate_app::Propose;
use solidate_app::core::{Hash, Variant};
use solidate_app::db::ProposalId;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::response::Response;
use topcoat::router::{Body, StatusCode, path_param_segment, query_params, route};

use super::{ApiError, ApiResult, api_ctx, doc_path, finish, json};
use crate::auth::app;

/// Sections needing propagation across the project's own documents.
#[route(GET "/api/v1/projects/{project}/sync")]
async fn api_queue(cx: &Cx) -> Result<Response> {
    finish(queue_inner(cx).await)
}

async fn queue_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let q = app(cx).sync_queue(&ctx, path_param_segment(cx, "project")).await?;
    Ok(json(StatusCode::OK, &q))
}

#[query_params]
struct ItemQuery {
    anchor: Option<String>,
}

/// Per-section sync status of one document; with `?anchor=`, the item needed to
/// propagate that section (both sides' text, base text, diffs, head hashes).
#[route(GET "/api/v1/projects/{project}/sync/{*path}")]
async fn api_doc(cx: &Cx) -> Result<Response> {
    finish(doc_inner(cx).await)
}

async fn doc_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let q = query_params::<ItemQuery>(cx).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(match q.anchor.as_deref() {
        Some(anchor) => json(StatusCode::OK, &app(cx).sync_item(&ctx, project, &path, anchor).await?),
        None => json(StatusCode::OK, &app(cx).doc_sync(&ctx, project, &path).await?),
    })
}

/// Exactly one of `anchors`, `all`, or `paired`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    #[serde(default)]
    anchors: Vec<String>,
    /// Every section needing attention.
    #[serde(default)]
    all: bool,
    /// Sections needing attention that exist in both variants.
    #[serde(default)]
    paired: bool,
}

/// Marks sections in sync without editing. Returns the document's sync status.
#[route(POST "/api/v1/projects/{project}/resolve/{*path}")]
async fn api_resolve(cx: &Cx, body: String) -> Result<Response> {
    finish(resolve_inner(cx, body).await)
}

async fn resolve_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let b: ResolveBody =
        serde_json::from_str(&body).map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))?;
    let app = app(cx);
    match (!b.anchors.is_empty(), b.all, b.paired) {
        (true, false, false) => {
            app.resolve_sync(&ctx, project, &path, &b.anchors).await?;
        }
        (false, true, false) => {
            let anchors: Vec<String> = app
                .doc_sync(&ctx, project, &path)
                .await?
                .sections
                .into_iter()
                .filter(|s| s.state.needs_attention())
                .map(|s| s.anchor)
                .collect();
            app.resolve_sync(&ctx, project, &path, &anchors).await?;
        }
        (false, false, true) => {
            app.resolve_paired_sync(&ctx, project, &path).await?;
        }
        _ => return Err(ApiError::bad_request("set exactly one of anchors, all, paired")),
    }
    Ok(json(StatusCode::OK, &app.doc_sync(&ctx, project, &path).await?))
}

/// The translation guide in effect for the project (`{source, content}`).
#[route(GET "/api/v1/projects/{project}/translation-guide")]
async fn api_guide(cx: &Cx) -> Result<Response> {
    finish(guide_inner(cx).await)
}

async fn guide_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let g = app(cx)
        .translation_guide(&ctx, path_param_segment(cx, "project"))
        .await?;
    Ok(json(StatusCode::OK, &g))
}

/// Open translation proposals on the project's own documents.
#[route(GET "/api/v1/projects/{project}/proposals")]
async fn api_proposals(cx: &Cx) -> Result<Response> {
    finish(proposals_inner(cx).await)
}

async fn proposals_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let ps = app(cx)
        .project_proposals(&ctx, path_param_segment(cx, "project"))
        .await?;
    Ok(json(StatusCode::OK, &ps))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeBody {
    /// The variant being translated into.
    variant: Variant,
    content: String,
    /// Content hash of that variant as last read; omit when it does not exist.
    base_hash: Option<Hash>,
    message: Option<String>,
    #[serde(default)]
    resolves: Vec<String>,
}

/// Submits a translation proposal for review. Body: JSON `{variant, content,
/// base_hash?, message?, resolves?}`. 201 with the proposal.
#[route(POST "/api/v1/projects/{project}/propose/{*path}")]
async fn api_propose(cx: &Cx, body: String) -> Result<Response> {
    finish(propose_inner(cx, body).await)
}

async fn propose_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let b: ProposeBody =
        serde_json::from_str(&body).map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))?;
    let content = b.content.replace("\r\n", "\n");
    let p = app(cx)
        .propose(
            &ctx,
            Propose {
                project,
                path: &path,
                variant: b.variant,
                content: &content,
                base: b.base_hash,
                message: b.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                resolves: &b.resolves,
            },
        )
        .await?;
    Ok(json(StatusCode::CREATED, &p))
}

/// Withdraws (rejects) a proposal. 204.
#[route(DELETE "/api/v1/projects/{project}/proposals/{id}")]
async fn api_reject(cx: &Cx) -> Result<Response> {
    finish(reject_inner(cx).await)
}

async fn reject_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let id: ProposalId = path_param_segment(cx, "id")
        .parse()
        .map_err(|_| ApiError::bad_request("invalid proposal id"))?;
    app(cx)
        .reject_proposal(&ctx, path_param_segment(cx, "project"), id)
        .await?;
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::NO_CONTENT;
    Ok(res)
}
