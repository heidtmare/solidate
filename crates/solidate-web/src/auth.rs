//! Request helpers for the current user and tenant, and session handlers.

use std::time::Duration;

use solidate_app::db::User;
use solidate_app::{App, Credential, Ctx};
use topcoat::Result;
use topcoat::context::{Cx, app_context, memoize};
use topcoat::cookie::{Cookie, Cookies, SameSite, cookies};
use topcoat::router::HeaderValue;
use topcoat::router::error::redirect;
use topcoat::router::request::uri;
use topcoat::session::{self, Token, TokenStore, TokenStoreFuture};

use crate::error::OrHttp;
use crate::ui::enc;

pub const SESSION_LIFETIME: Duration = Duration::from_secs(14 * 24 * 3600);

pub fn app(cx: &Cx) -> &App {
    app_context::<App>(cx)
}

/// The credential in an `Authorization` header, if it uses a supported scheme.
pub fn credential(authorization: Option<&HeaderValue>) -> Option<Credential<'_>> {
    authorization
        .and_then(|v| v.to_str().ok())
        .and_then(Credential::from_authorization)
}

/// `WWW-Authenticate` value for `401` responses.
pub fn challenge() -> HeaderValue {
    HeaderValue::from_static(Credential::CHALLENGE)
}

#[memoize(as_ref)]
async fn session_user(cx: &Cx) -> Option<User> {
    let hash = session::token_hash(cx).await.ok().flatten()?;
    app(cx).session_user(hash.as_slice()).await.ok().flatten()
}

pub async fn current_user(cx: &Cx) -> Option<&User> {
    session_user(cx).await
}

/// The signed-in user, or a redirect to the login page.
pub async fn require_user(cx: &Cx) -> Result<&User> {
    match current_user(cx).await {
        Some(u) => Ok(u),
        None => {
            let here = uri(cx).path_and_query().map_or("/", |p| p.as_str());
            Err(redirect(format!("/login?next={}", enc(here))).into())
        }
    }
}

/// The signed-in user's context in `tenant`.
pub async fn tenant_ctx(cx: &Cx, tenant: &str) -> Result<Ctx> {
    let user = require_user(cx).await?;
    app(cx).user_ctx(tenant, user.id).await.or_http()
}

pub async fn start_session(cx: &Cx, user: &User) -> Result<()> {
    let s = session::start(cx).await?;
    let expires = time::OffsetDateTime::from(s.expires_at);
    app(cx)
        .start_session(s.token_hash.as_slice(), user.id, expires)
        .await
        .or_http()
}

pub async fn end_session(cx: &Cx) -> Result<()> {
    if let Some(hash) = session::stop(cx).await? {
        app(cx).end_session(hash.as_slice()).await.or_http()?;
    }
    Ok(())
}

/// Session cookie without `Secure` and the `__Host-` prefix, for plain-HTTP
/// development on hosts browsers do not treat as secure.
pub struct InsecureCookieTokenStore;

const INSECURE_COOKIE: &str = "solidate_session";

fn insecure_jar(cx: &Cx) -> impl Cookies {
    cookies(cx)
        .override_same_site(SameSite::Lax)
        .override_http_only(true)
        .override_path("/")
}

impl TokenStore for InsecureCookieTokenStore {
    fn read<'a>(&'a self, cx: &'a Cx) -> TokenStoreFuture<'a, Option<Token>> {
        Box::pin(async move {
            Ok(insecure_jar(cx)
                .get(INSECURE_COOKIE)
                .and_then(|c| Token::decode(c.value_trimmed()).ok()))
        })
    }

    fn write<'a>(&'a self, cx: &'a Cx, token: Token, max_age: Duration) -> TokenStoreFuture<'a, ()> {
        Box::pin(async move {
            let max_age = topcoat::cookie::time::Duration::try_from(max_age)?;
            insecure_jar(cx)
                .override_max_age(max_age)
                .add(Cookie::new(INSECURE_COOKIE, token.encode()));
            Ok(())
        })
    }

    fn delete<'a>(&'a self, cx: &'a Cx) -> TokenStoreFuture<'a, ()> {
        Box::pin(async move {
            insecure_jar(cx).remove(Cookie::new(INSECURE_COOKIE, ""));
            Ok(())
        })
    }
}
