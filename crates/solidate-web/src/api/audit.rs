//! Tenant audit log (admin scope).

use serde::Serialize;
use solidate_app::db::{AuditEntry, AuditId};
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::response::Response;
use topcoat::router::{StatusCode, query_params, route};

use super::{ApiError, ApiResult, api_ctx, finish, json};
use crate::auth::app;

#[query_params]
struct AuditQuery {
    limit: Option<i64>,
    /// Continue after this entry id (the previous page's `next`).
    before: Option<String>,
}

/// Audit entries newest first. `next` is the `before` value for the following
/// page, `null` on the last page.
#[route(GET "/api/v1/audit")]
async fn api_audit(cx: &Cx) -> Result<Response> {
    finish(audit_inner(cx).await)
}

async fn audit_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let q = query_params::<AuditQuery>(cx).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let before = q
        .before
        .as_deref()
        .map(str::parse::<AuditId>)
        .transpose()
        .map_err(|_| ApiError::bad_request("before must be an audit entry id"))?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let entries = app(cx).audit_log(&ctx, limit, before).await?;
    #[derive(Serialize)]
    struct Out {
        next: Option<AuditId>,
        entries: Vec<AuditEntry>,
    }
    let next = (entries.len() as i64 == limit)
        .then(|| entries.last().map(|e| e.id))
        .flatten();
    Ok(json(StatusCode::OK, &Out { next, entries }))
}
