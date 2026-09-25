//! Solidate server: server-rendered UI on Topcoat.

mod assets;
mod auth;
mod error;
mod layout;
mod pages;
mod ui;

use solidate_app::App;
use topcoat::cookie::RouterBuilderCookieExt;
use topcoat::router::{Router, RouterBuilderDiscoverExt};
use topcoat::session::{RouterBuilderSessionExt, SessionConfig, cookie::CookieTokenStore};

#[derive(Debug, Clone, Default)]
pub struct WebConfig {
    /// Use a non-`Secure` session cookie (plain-HTTP development only).
    pub insecure_cookies: bool,
}

pub fn router(app: App, web: WebConfig) -> Router {
    let sessions = SessionConfig::builder().lifetime(auth::SESSION_LIFETIME);
    let sessions = if web.insecure_cookies {
        sessions.token_store(auth::InsecureCookieTokenStore)
    } else {
        sessions.token_store(CookieTokenStore::new())
    };
    Router::builder()
        .discover()
        .cookies()
        .sessions(sessions.build())
        .app_context(app)
        .build()
}
