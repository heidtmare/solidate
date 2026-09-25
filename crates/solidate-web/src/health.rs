//! `GET /healthz`: 200 when the database answers, 503 otherwise. Unauthenticated.

use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::response::Response;
use topcoat::router::route;
use topcoat::router::{Body, StatusCode};

use crate::auth::app;

#[route(GET "/healthz")]
async fn healthz(cx: &Cx) -> Result<Response> {
    let (status, body) = match app(cx).db().ping().await {
        Ok(()) => (StatusCode::OK, "ok"),
        Err(e) => {
            tracing::error!(error = %e, "health check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    };
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    Ok(res)
}
