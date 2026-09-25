//! Static assets compiled into the binary, served with content-hashed URLs.

use std::sync::OnceLock;

use topcoat::Result;
use topcoat::router::{HeaderValue, header, route};

const CSS: &str = include_str!("../static/app.css");
const HTMX: &str = include_str!("../static/htmx.min.js");
const APP_JS: &str = include_str!("../static/app.js");
const MERMAID: &str = include_str!("../static/mermaid.min.js");

fn versioned(path: &str, body: &str) -> String {
    format!("{path}?v={}", &blake3::hash(body.as_bytes()).to_hex()[..12])
}

pub fn css_url() -> &'static str {
    static U: OnceLock<String> = OnceLock::new();
    U.get_or_init(|| versioned("/static/app.css", CSS))
}

pub fn htmx_url() -> &'static str {
    static U: OnceLock<String> = OnceLock::new();
    U.get_or_init(|| versioned("/static/htmx.min.js", HTMX))
}

pub fn app_js_url() -> &'static str {
    static U: OnceLock<String> = OnceLock::new();
    U.get_or_init(|| versioned("/static/app.js", APP_JS))
}

pub fn mermaid_url() -> &'static str {
    static U: OnceLock<String> = OnceLock::new();
    U.get_or_init(|| versioned("/static/mermaid.min.js", MERMAID))
}

type Asset = ([(header::HeaderName, HeaderValue); 2], &'static str);

fn asset(content_type: &'static str, body: &'static str) -> Asset {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ],
        body,
    )
}

#[route(GET "/static/app.css")]
async fn app_css() -> Result<Asset> {
    Ok(asset("text/css; charset=utf-8", CSS))
}

#[route(GET "/static/htmx.min.js")]
async fn htmx_js() -> Result<Asset> {
    Ok(asset("text/javascript; charset=utf-8", HTMX))
}

#[route(GET "/static/app.js")]
async fn app_js() -> Result<Asset> {
    Ok(asset("text/javascript; charset=utf-8", APP_JS))
}

#[route(GET "/static/mermaid.min.js")]
async fn mermaid_js() -> Result<Asset> {
    Ok(asset("text/javascript; charset=utf-8", MERMAID))
}
