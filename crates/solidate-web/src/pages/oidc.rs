//! OpenID Connect sign-in: redirect to the provider and handle its callback.
//!
//! The attempt's state, nonce, PKCE verifier and post-login path travel in a
//! short-lived `HttpOnly` cookie scoped to `/login/oidc`. The state binds the
//! callback to the browser that started the attempt.

use std::time::Duration;

use percent_encoding::percent_decode_str;
use solidate_app::{AppError, PendingLogin};
use topcoat::Result;
use topcoat::context::{Cx, app_context};
use topcoat::cookie::{Cookie, Cookies, SameSite, cookies};
use topcoat::router::error::{SeeOther, not_found, see_other};
use topcoat::router::{StatusCode, page, query_params, route};
use topcoat::view::{View, view};

use super::home::{login_form, sso_label};
use crate::WebConfig;
use crate::auth::{app, start_session};
use crate::ui::{enc, safe_next};

const COOKIE: &str = "solidate_oidc";
const COOKIE_PATH: &str = "/login/oidc";
const COOKIE_LIFETIME: Duration = Duration::from_secs(600);

#[query_params]
struct StartQuery {
    next: Option<String>,
}

#[query_params]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Cookie value: `state.nonce.verifier.next`. The first three are base64url;
/// `next` is percent-encoded, so none contain `.`.
fn encode(p: &PendingLogin, next: &str) -> String {
    format!("{}.{}.{}.{}", p.state, p.nonce, p.pkce_verifier, enc(next))
}

fn decode(value: &str) -> Option<(PendingLogin, String)> {
    let mut parts = value.splitn(4, '.');
    let pending = PendingLogin {
        state: parts.next()?.to_owned(),
        nonce: parts.next()?.to_owned(),
        pkce_verifier: parts.next()?.to_owned(),
    };
    let next = percent_decode_str(parts.next()?).decode_utf8().ok()?;
    Some((pending, safe_next(Some(&next))))
}

fn jar(cx: &Cx) -> impl Cookies {
    let secure = !app_context::<WebConfig>(cx).insecure_cookies;
    cookies(cx).map(move |c| {
        c.set_path(COOKIE_PATH);
        c.set_http_only(true);
        // `Lax` so the cookie accompanies the provider's top-level redirect back.
        c.set_same_site(SameSite::Lax);
        c.set_secure(secure);
    })
}

#[route(GET "/login/oidc")]
async fn oidc_start(cx: &Cx) -> Result<SeeOther> {
    let oidc = app(cx).oidc().ok_or_else(not_found)?;
    let next = safe_next(query_params::<StartQuery>(cx).ok().and_then(|q| q.next.as_deref()));
    let start = oidc.begin().map_err(crate::error::http)?;
    let mut cookie = Cookie::new(COOKIE, encode(&start.pending, &next));
    cookie.set_max_age(topcoat::cookie::time::Duration::try_from(COOKIE_LIFETIME)?);
    jar(cx).add(cookie);
    Ok(see_other(start.url))
}

#[page("/login/oidc/callback")]
async fn oidc_callback(cx: &Cx) -> Result<impl View> {
    let oidc = app(cx).oidc().ok_or_else(not_found)?;
    let jar = jar(cx);
    let attempt = jar.get(COOKIE).and_then(|c| decode(c.value()));
    jar.remove(Cookie::new(COOKIE, ""));
    let q = query_params::<CallbackQuery>(cx).ok();

    let failed = |next: String, message: String| {
        Ok(view! {
            (StatusCode::UNAUTHORIZED)
            login_form(next: next, error: Some(message), sso: sso_label(cx))
        })
    };
    let generic = format!("Sign-in with {} failed.", oidc.label());
    let Some((pending, next)) = attempt else {
        return failed("/".to_owned(), generic);
    };
    let (code, state) = match q {
        Some(CallbackQuery {
            code: Some(code),
            state: Some(state),
            error: None,
        }) => (code, state),
        q => {
            let error = q.and_then(|q| q.error.as_deref());
            tracing::info!(target: "solidate::auth", error, "oidc callback without code");
            return failed(next, generic);
        }
    };
    let result = match oidc.finish(&pending, code, state).await {
        Ok(identity) => app(cx).oidc_login(&identity).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(user) => {
            start_session(cx, &user).await?;
            Err(see_other(next).into())
        }
        Err(AppError::Unauthorized) => failed(next, generic),
        Err(AppError::Invalid(m)) => failed(next, format!("{generic} {}", capitalize(&m))),
        Err(e) => Err(crate::error::http(e)),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).chain(std::iter::once('.')).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_round_trip() {
        let p = PendingLogin {
            state: "s-_1".into(),
            nonce: "n".into(),
            pkce_verifier: "v".into(),
        };
        let (back, next) = decode(&encode(&p, "/t/acme?q=a.b&x=1")).unwrap();
        assert_eq!(back, p);
        assert_eq!(next, "/t/acme?q=a.b&x=1");
        let (_, next) = decode(&encode(&p, "//evil.example")).unwrap();
        assert_eq!(next, "/");
        assert!(decode("a.b").is_none());
    }
}
