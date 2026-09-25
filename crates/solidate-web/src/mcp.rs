//! Streamable HTTP MCP endpoint at `/mcp`, authenticated with bearer API tokens.

use std::convert::Infallible;

use solidate_app::App;
use topcoat::router::request::Request;
use topcoat::router::response::Response;
use topcoat::router::tower::TowerRoute;
use topcoat::router::{Body, HeaderValue, StatusCode, header};

fn unauthorized() -> Response {
    let mut res = Response::new(Body::from(
        r#"{"error":{"code":"unauthorized","message":"a valid API token is required"}}"#,
    ));
    *res.status_mut() = StatusCode::UNAUTHORIZED;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
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
            let token = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::to_owned);
            let ctx = match token {
                Some(t) => app.token_ctx(&t).await.ok(),
                None => None,
            };
            let Some(ctx) = ctx else {
                return Ok::<_, Infallible>(unauthorized());
            };
            req.extensions_mut().insert(ctx);
            let res = tower::Service::call(&mut mcp, req).await?;
            Ok(res.map(Body::new))
        }
    });
    TowerRoute::any("/mcp", svc)
}
