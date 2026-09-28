//! Doc↔code drift: source reports, the drift queue, per-document drift, reverse
//! lookup and verification.

use serde::Deserialize;
use solidate_app::SourceReport;
use solidate_app::core::Variant;
use solidate_app::core::sources::Files;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::response::Response;
use topcoat::router::{StatusCode, path_param_segment, route};

use super::{ApiError, ApiResult, api_ctx, doc_path, finish, json, parse_variant};
use crate::auth::app;

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportBody {
    revision: Option<String>,
    /// Path → git blob object id.
    #[serde(default)]
    files: Files,
    #[serde(default)]
    removed: Vec<String>,
    #[serde(default)]
    replace: bool,
}

/// Records repository file hashes. Body: JSON `{revision?, files, removed?, replace?}`.
/// Returns `{revision, reported_at, files}` (`files`: the project's file count).
#[route(POST "/api/v1/projects/{project}/sources")]
async fn api_report(cx: &Cx, body: String) -> Result<Response> {
    finish(report_inner(cx, body).await)
}

async fn report_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let b: ReportBody = parse(&body)?;
    let s = app(cx)
        .report_sources(
            &ctx,
            path_param_segment(cx, "project"),
            SourceReport {
                revision: b.revision.as_deref(),
                files: &b.files,
                removed: &b.removed,
                replace: b.replace,
            },
        )
        .await?;
    Ok(json(StatusCode::OK, &s))
}

/// Bound sections needing attention across the project's own documents.
#[route(GET "/api/v1/projects/{project}/drift")]
async fn api_queue(cx: &Cx) -> Result<Response> {
    finish(queue_inner(cx).await)
}

async fn queue_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let q = app(cx).drift_queue(&ctx, path_param_segment(cx, "project")).await?;
    Ok(json(StatusCode::OK, &q))
}

/// Drift status of every bound section of one document.
#[route(GET "/api/v1/projects/{project}/drift/{*path}")]
async fn api_doc(cx: &Cx) -> Result<Response> {
    finish(doc_inner(cx).await)
}

async fn doc_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    Ok(json(StatusCode::OK, &app(cx).doc_drift(&ctx, project, &path).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AffectedBody {
    paths: Vec<String>,
}

/// Bound sections whose patterns match any of `paths`. Body: JSON `{paths}`.
#[route(POST "/api/v1/projects/{project}/affected")]
async fn api_affected(cx: &Cx, body: String) -> Result<Response> {
    finish(affected_inner(cx, body).await)
}

async fn affected_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let b: AffectedBody = parse(&body)?;
    let a = app(cx)
        .affected_sections(&ctx, path_param_segment(cx, "project"), &b.paths)
        .await?;
    Ok(json(StatusCode::OK, &a))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextBody {
    paths: Vec<String>,
    /// Preferred variant, `ai` (default) or `human`.
    variant: Option<String>,
}

/// Bound sections whose patterns match any of `paths`, with their content.
/// Body: JSON `{paths, variant?}`.
#[route(POST "/api/v1/projects/{project}/context")]
async fn api_context(cx: &Cx, body: String) -> Result<Response> {
    finish(context_inner(cx, body).await)
}

async fn context_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let b: ContextBody = parse(&body)?;
    let prefer = match b.variant.as_deref() {
        None => Variant::Ai,
        v => parse_variant(v)?,
    };
    let c = app(cx)
        .section_context(&ctx, path_param_segment(cx, "project"), &b.paths, prefer)
        .await?;
    Ok(json(StatusCode::OK, &c))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyBody {
    anchors: Vec<String>,
    /// Report revision the caller checked against.
    revision: Option<String>,
}

/// Marks bound sections as verified against the current report. Body: JSON
/// `{anchors, revision?}`. Returns the document's drift status.
#[route(POST "/api/v1/projects/{project}/verify/{*path}")]
async fn api_verify(cx: &Cx, body: String) -> Result<Response> {
    finish(verify_inner(cx, body).await)
}

async fn verify_inner(cx: &Cx, body: String) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let (project, path) = (path_param_segment(cx, "project"), doc_path(cx)?);
    let b: VerifyBody = parse(&body)?;
    let d = app(cx)
        .verify_sources(&ctx, project, &path, &b.anchors, b.revision.as_deref())
        .await?;
    Ok(json(StatusCode::OK, &d))
}
