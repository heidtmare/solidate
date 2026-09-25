//! In-process HTTP tests through `Router::handle`. Require `DATABASE_URL`.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use solidate_app::core::{DocPath, Role, Slug, Variant};
use solidate_app::db::{Db, Expect};
use solidate_app::{App, Config, PutDoc};
use solidate_web::{WebConfig, router};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use topcoat::router::{Body, Router, StatusCode, header, to_bytes};

struct Client {
    router: Router,
    cookie: Option<String>,
}

struct Reply {
    status: StatusCode,
    location: Option<String>,
    body: String,
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{k}={}", utf8_percent_encode(v, NON_ALPHANUMERIC)))
        .collect::<Vec<_>>()
        .join("&")
}

impl Client {
    async fn send(&mut self, method: &str, uri: &str, body: Option<String>) -> Reply {
        let mut req = topcoat::router::request::Request::builder().method(method).uri(uri);
        if let Some(c) = &self.cookie {
            req = req.header(header::COOKIE, c);
        }
        if body.is_some() {
            req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        }
        let res = self
            .router
            .handle(req.body(body.map_or_else(Body::empty, Body::from)).unwrap())
            .await;
        if let Some(set) = res.headers().get(header::SET_COOKIE) {
            let pair = set.to_str().unwrap().split(';').next().unwrap().to_owned();
            self.cookie = Some(pair);
        }
        let status = res.status();
        let location = res
            .headers()
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_owned());
        let body = String::from_utf8(to_bytes(res.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
        Reply { status, location, body }
    }

    async fn get(&mut self, uri: &str) -> Reply {
        self.send("GET", uri, None).await
    }

    async fn post(&mut self, uri: &str, fields: &[(&str, &str)]) -> Reply {
        self.send("POST", uri, Some(form(fields))).await
    }
}

async fn setup(pool: PgPoolOptions, opts: PgConnectOptions) -> (App, Client) {
    let app = App::new(Db::connect_with(pool, opts).await.unwrap(), Config::default());
    app.create_tenant(&Slug::parse("acme").unwrap(), "Acme").await.unwrap();
    let sys = app.system_ctx("acme").await.unwrap();
    app.create_project(&sys, &Slug::parse("app").unwrap(), "App", None)
        .await
        .unwrap();
    let user = app
        .register_user("ada@acme.dev", "Ada", "long-enough-pw")
        .await
        .unwrap();
    app.set_member(&sys, user.id, Role::Editor).await.unwrap();
    app.put_doc(
        &sys,
        PutDoc {
            project: "app",
            path: &DocPath::parse("design/auth").unwrap(),
            variant: Variant::Human,
            content: "# Auth\n\nOpaque <b>tokens</b>.\n",
            expect: Expect::Absent,
            message: None,
            resolves: &[],
        },
    )
    .await
    .unwrap();
    let client = Client {
        router: router(app.clone(), WebConfig { insecure_cookies: true }),
        cookie: None,
    };
    (app, client)
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn login_view_edit_conflict_logout(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (_app, mut c) = setup(pool, opts).await;

    let r = c.get("/t/acme").await;
    assert_eq!(r.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(r.location.as_deref(), Some("/login?next=%2Ft%2Facme"));

    let r = c
        .post(
            "/login",
            &[("email", "ada@acme.dev"), ("password", "wrong-password"), ("next", "/")],
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.body.contains("Invalid email or password."));

    // Open redirects are rejected.
    let r = c
        .post(
            "/login",
            &[
                ("email", "ada@acme.dev"),
                ("password", "long-enough-pw"),
                ("next", "//evil"),
            ],
        )
        .await;
    assert_eq!((r.status, r.location.as_deref()), (StatusCode::SEE_OTHER, Some("/")));

    let r = c.get("/t/acme/p/app/d/design/auth").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains(r#"<h1 id="auth">"#));
    assert!(!r.body.contains("<b>tokens</b>"), "raw HTML is not rendered");

    let edit = c.get("/t/acme/p/app/edit/design/auth").await;
    let base = edit.body.split(r#"name="base" value=""#).nth(1).unwrap()[..64].to_owned();
    let r = c
        .post(
            "/t/acme/p/app/edit/design/auth",
            &[("content", "# Auth\n\nRotated.\n"), ("base", &base), ("message", "")],
        )
        .await;
    assert_eq!(
        (r.status, r.location.as_deref()),
        (StatusCode::SEE_OTHER, Some("/t/acme/p/app/d/design/auth"))
    );

    // Same base again: conflict page keeps the submitted text.
    let r = c
        .post(
            "/t/acme/p/app/edit/design/auth",
            &[("content", "# Auth\n\nMine.\n"), ("base", &base), ("message", "")],
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("changed by someone else") && r.body.contains("Mine."));

    let r = c.get("/t/acme/p/app/sync").await;
    assert!(r.body.contains("human_ahead"));

    let r = c.get("/t/acme/p/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.body.contains("Not found"));

    let r = c.post("/logout", &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(c.get("/t/acme").await.status, StatusCode::TEMPORARY_REDIRECT);
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn non_member_is_forbidden(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, mut c) = setup(pool, opts).await;
    app.register_user("eve@other.dev", "Eve", "long-enough-pw")
        .await
        .unwrap();
    c.post(
        "/login",
        &[
            ("email", "eve@other.dev"),
            ("password", "long-enough-pw"),
            ("next", "/"),
        ],
    )
    .await;
    let r = c.get("/t/acme/p/app").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}
