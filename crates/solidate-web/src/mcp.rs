//! Streamable HTTP MCP endpoint at `/mcp`, authenticated like the REST API.

use std::convert::Infallible;

use solidate_app::{App, AppError};
use topcoat::router::request::Request;
use topcoat::router::response::Response;
use topcoat::router::tower::TowerRoute;
use topcoat::router::{Body, HeaderValue, StatusCode, header};

use crate::auth::{challenge, credential};

fn error_response(status: StatusCode, body: &'static str) -> Response {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}

fn unauthorized() -> Response {
    let mut res = error_response(
        StatusCode::UNAUTHORIZED,
        r#"{"error":{"code":"unauthorized","message":"a valid API token is required"}}"#,
    );
    res.headers_mut().insert(header::WWW_AUTHENTICATE, challenge());
    res
}

fn rate_limited(retry_after_secs: u64) -> Response {
    let mut res = error_response(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"code":"rate_limited","message":"rate limit exceeded"}}"#,
    );
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
    res
}

/// Accepted `Host` values: loopback plus the host of `public_url`.
pub fn allowed_hosts(public_url: Option<&str>) -> Vec<String> {
    let mut hosts = vec!["localhost".to_owned(), "127.0.0.1".to_owned(), "::1".to_owned()];
    if let Some(authority) = public_url
        .and_then(|u| u.split_once("://"))
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
    {
        hosts.push(authority.to_owned());
        if let Some((host, _port)) = authority.rsplit_once(':') {
            hosts.push(host.to_owned());
        }
    }
    hosts
}

pub fn route(
    app: App,
    public_url: Option<&str>,
) -> TowerRoute<
    impl tower::Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone + Send + Sync + 'static,
> {
    let mcp = solidate_mcp::http_service(app.clone(), allowed_hosts(public_url));
    let svc = tower::service_fn(move |mut req: Request| {
        let (app, mut mcp) = (app.clone(), mcp.clone());
        async move {
            let ctx = match credential(req.headers().get(header::AUTHORIZATION)) {
                Some(c) => app.authenticate(c).await,
                None => Err(AppError::Unauthorized),
            };
            let ctx = match ctx {
                Ok(ctx) => ctx,
                Err(AppError::RateLimited { retry_after_secs }) => {
                    return Ok::<_, Infallible>(rate_limited(retry_after_secs));
                }
                Err(AppError::Internal(m)) => {
                    tracing::error!(error = %m, "mcp authentication failed");
                    return Ok(error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        r#"{"error":{"code":"internal","message":"internal error"}}"#,
                    ));
                }
                Err(_) => return Ok(unauthorized()),
            };
            req.extensions_mut().insert(ctx);
            let res = tower::Service::call(&mut mcp, req).await?;
            Ok(res.map(Body::new))
        }
    });
    TowerRoute::any("/mcp", svc)
}
