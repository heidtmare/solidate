//! Sign-in, sign-out, tenant selection, tenant home.

use serde::Deserialize;
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::content::Form;
use topcoat::router::error::{SeeOther, redirect, see_other};
use topcoat::router::path_param_segment;
use topcoat::router::{StatusCode, page, query_params, route};
use topcoat::view::{View, component, view};

use crate::auth::{app, end_session, require_user, start_session, tenant_ctx};
use crate::error::OrHttp;
use crate::ui::{enc, project_url, safe_next, tenant_url};

#[page("/")]
async fn home(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let tenants = app(cx)
        .db()
        .user_tenants(user.id)
        .await
        .map_err(solidate_app::AppError::from)
        .or_http()?;
    if let [only] = tenants.as_slice() {
        return Err(redirect(tenant_url(&only.tenant.slug)).into());
    }
    Ok(view! {
        <h1>"Workspaces"</h1>
        if tenants.is_empty() {
            <p class="muted">"You are not a member of any workspace. Ask an administrator to add you."</p>
        }
        <ul class="list">
            for m in &tenants {
                <li>
                    <a href=(tenant_url(&m.tenant.slug))>(m.tenant.name.as_str())</a>
                    <span class="badge">(m.role.as_str())</span>
                </li>
            }
        </ul>
    })
}

#[query_params]
struct LoginQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    next: Option<String>,
}

/// `sso` is the OpenID Connect provider label when that sign-in is enabled.
#[component]
pub(super) async fn login_form(
    next: String,
    #[default] error: Option<String>,
    #[default] email: String,
    #[default] sso: Option<String>,
) -> Result<impl View> {
    let sso_href = format!("/login/oidc?next={}", enc(&next));
    Ok(view! {
        <section class="narrow">
            <h1>"Sign in"</h1>
            match error {
                Some(e) => <p class="alert">(e)</p>,
                None => "",
            }
            match sso {
                Some(label) => <p><a class="button" href=(sso_href)>"Sign in with " (label)</a></p>,
                None => "",
            }
            <form method="post" action="/login" class="stack">
                <input type="hidden" name="next" value=(next)>
                <label>"Email" <input type="email" name="email" value=(email) required="" autofocus=""></label>
                <label>"Password" <input type="password" name="password" required=""></label>
                <button type="submit">"Sign in"</button>
            </form>
        </section>
    })
}

pub(super) fn sso_label(cx: &Cx) -> Option<String> {
    app(cx).oidc().map(|o| o.label().to_owned())
}

#[page("/login")]
async fn login(cx: &Cx) -> Result<impl View> {
    let q = query_params::<LoginQuery>(cx).ok();
    let next = safe_next(q.and_then(|q| q.next.as_deref()));
    let sso = sso_label(cx);
    Ok(view! { login_form(next: next, sso: sso) })
}

#[page(POST "/login")]
async fn login_submit(cx: &Cx, Form(form): Form<LoginForm>) -> Result<impl View> {
    let next = safe_next(form.next.as_deref());
    match app(cx).login(&form.email, &form.password).await {
        Ok(user) => {
            start_session(cx, &user).await?;
            Err(see_other(next).into())
        }
        Err(solidate_app::AppError::Unauthorized) => Ok(view! {
            (StatusCode::UNAUTHORIZED)
            login_form(next: next, error: Some("Invalid email or password.".to_owned()), email: form.email, sso: sso_label(cx))
        }),
        Err(e) => Err(crate::error::http(e)),
    }
}

#[route(POST "/logout")]
async fn logout(cx: &Cx) -> Result<SeeOther> {
    end_session(cx).await?;
    Ok(see_other("/login"))
}

#[page("/t/{tenant}")]
async fn tenant_home(cx: &Cx) -> Result<impl View> {
    let tenant = path_param_segment(cx, "tenant");
    let ctx = tenant_ctx(cx, tenant).await?;
    let projects = app(cx).projects(&ctx).await.or_http()?;
    let t = ctx.tenant.slug.clone();
    Ok(view! {
        <nav class="crumbs"><span>(ctx.tenant.name.as_str())</span></nav>
        <h1>"Projects"</h1>
        <form method="get" action=(format!("/t/{t}/search")) class="search">
            <input type="search" name="q" placeholder="Search all projects">
        </form>
        <ul class="list">
            for p in &projects {
                let parent = p.parent_id.and_then(|id| projects.iter().find(|x| x.id == id)).map(|x| x.slug.clone());
                <li>
                    <a href=(project_url(&t, &p.slug))>(p.name.as_str())</a>
                    <code class="muted">(p.slug.as_str())</code>
                    match parent {
                        Some(parent) => <span class="badge">"inherits " (parent)</span>,
                        None => "",
                    }
                </li>
            }
        </ul>
    })
}
