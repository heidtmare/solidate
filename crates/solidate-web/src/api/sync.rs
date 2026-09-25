//! Sync queue, per-document status and items, resolution.

use serde::Deserialize;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::response::Response;
use topcoat::router::{StatusCode, path_param_segment, query_params, route};

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
