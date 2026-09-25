//! Solidate server: server-rendered UI on Topcoat.

mod api;
mod assets;
mod auth;
mod error;
mod health;
mod layout;
mod mcp;
mod pages;
mod trace;
mod ui;

use solidate_app::App;
use topcoat::cookie::RouterBuilderCookieExt;
use topcoat::router::{Router, RouterBuilderDiscoverExt};
use topcoat::session::{RouterBuilderSessionExt, SessionConfig, cookie::CookieTokenStore};

#[derive(Debug, Clone, Default)]
pub struct WebConfig {
    /// Use a non-`Secure` session cookie (plain-HTTP development only).
    pub insecure_cookies: bool,
    /// Absolute base URL (no trailing slash) used in `llms.txt` links. Relative
    /// links when `None`.
    pub public_url: Option<String>,
}

pub fn router(app: App, web: WebConfig) -> Router {
    let sessions = SessionConfig::builder().lifetime(auth::SESSION_LIFETIME);
    let sessions = if web.insecure_cookies {
        sessions.token_store(auth::InsecureCookieTokenStore)
    } else {
        sessions.token_store(CookieTokenStore::new())
    };
    let mcp = mcp::route(app.clone(), web.public_url.as_deref());
    Router::builder()
        .discover()
        .route(mcp)
        .layer(trace::TraceLayer)
        .cookies()
        .sessions(sessions.build())
        .app_context(app)
        .app_context(web)
        .build()
}
