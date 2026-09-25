//! Request tracing: one span per request (method, path) and one event on
//! completion with status and latency. App-layer events inherit the span.

use std::time::Instant;

use topcoat::context::Cx;
use topcoat::router::error::{
    BadRequestError, ContentTooLargeError, ForbiddenError, MethodNotAllowedError, NotFoundError, RedirectError,
    SeeOther, ServiceUnavailableError, TooManyRequestsError, UnauthorizedError,
};
use topcoat::router::request::{method, uri};
use topcoat::router::{Body, Layer, LayerFuture, Next, Path};
use tracing::Instrument;

pub struct TraceLayer;

/// Status of a framework error, as the router will render it.
fn error_status(e: &topcoat::Error) -> u16 {
    if e.is::<RedirectError>() || e.is::<SeeOther>() {
        303
    } else if e.is::<BadRequestError>() {
        400
    } else if e.is::<UnauthorizedError>() {
        401
    } else if e.is::<ForbiddenError>() {
        403
    } else if e.is::<NotFoundError>() {
        404
    } else if e.is::<MethodNotAllowedError>() {
        405
    } else if e.is::<ContentTooLargeError>() {
        413
    } else if e.is::<TooManyRequestsError>() {
        429
    } else if e.is::<ServiceUnavailableError>() {
        503
    } else {
        500
    }
}

impl Layer for TraceLayer {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        let span = tracing::info_span!("request", method = %method(cx), path = uri(cx).path());
        Box::pin(
            async move {
                let start = Instant::now();
                let res = next.run(cx, body).await;
                let status = match &res {
                    Ok(r) => r.status().as_u16(),
                    Err(e) => error_status(e),
                };
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                if status >= 500 {
                    tracing::error!(status, ms, "response");
                } else {
                    tracing::info!(status, ms, "response");
                }
                res
            }
            .instrument(span),
        )
    }
}
