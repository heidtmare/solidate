//! HTTP API tests through `Router::handle`. Require `DATABASE_URL`.

use serde_json::Value;
use solidate_app::core::{Scope, Slug};
use solidate_app::db::Db;
use solidate_app::{App, Config};
use solidate_web::{WebConfig, router};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use topcoat::router::{Body, Router, StatusCode, header, to_bytes};

struct Reply {
    status: StatusCode,
    etag: Option<String>,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

async fn call(r: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<&str>) -> Reply {
    let mut req = topcoat::router::request::Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = r
        .handle(
            req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned())))
                .unwrap(),
        )
        .await;
    let status = res.status();
    let etag = res.headers().get(header::ETAG).map(|v| v.to_str().unwrap().to_owned());
    let body = String::from_utf8(to_bytes(res.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
    Reply { status, etag, body }
}

struct Api {
    router: Router,
    auth: String,
}

impl Api {
    async fn req(&self, method: &str, uri: &str, extra: &[(&str, &str)], body: Option<&str>) -> Reply {
        let mut h = vec![("authorization", self.auth.as_str())];
        h.extend_from_slice(extra);
        call(&self.router, method, uri, &h, body).await
    }
}

async fn setup(pool: PgPoolOptions, opts: PgConnectOptions) -> (App, Router, String, String) {
    let app = App::new(Db::connect_with(pool, opts).await.unwrap(), Config::default());
    app.create_tenant(&Slug::parse("acme").unwrap(), "Acme").await.unwrap();
    let sys = app.system_ctx("acme").await.unwrap();
    let shared = app
        .create_project(&sys, &Slug::parse("shared").unwrap(), "Shared", None)
        .await
        .unwrap();
    app.create_project(&sys, &Slug::parse("app").unwrap(), "App", Some("shared"))
        .await
        .unwrap();
    let rw = app
        .create_api_token(&sys, "rw", &[Scope::Write], None, None)
        .await
        .unwrap();
    let ro = app
        .create_api_token(&sys, "ro", &[Scope::Read], Some(shared.id), None)
        .await
        .unwrap();
    let router = router(
        app.clone(),
        WebConfig {
            insecure_cookies: true,
            public_url: Some("https://docs.example".into()),
        },
    );
    (
        app,
        router,
        format!("Bearer {}", rw.secret),
        format!("Bearer {}", ro.secret),
    )
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn auth_and_projects(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (_app, router, rw, ro) = setup(pool, opts).await;

    let r = call(&router, "GET", "/api/v1/projects", &[], None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json()["error"]["code"], "unauthorized");
    let r = call(
        &router,
        "GET",
        "/api/v1/projects",
        &[("authorization", "Bearer sol_000000000000_00")],
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let api = Api {
        router: router.clone(),
        auth: rw,
    };
    let who = api.req("GET", "/api/v1", &[], None).await.json();
    assert_eq!(
        (who["tenant"].as_str(), who["scopes"][0].as_str()),
        (Some("acme"), Some("write"))
    );
    let list = api.req("GET", "/api/v1/projects", &[], None).await.json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    let app_p = api.req("GET", "/api/v1/projects/app", &[], None).await.json();
    assert_eq!(app_p["inherits"][0], "shared");

    // Restricted read-only token sees one project and cannot write.
    let limited = Api { router, auth: ro };
    let list = limited.req("GET", "/api/v1/projects", &[], None).await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let r = limited.req("GET", "/api/v1/projects/app/tree", &[], None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = limited
        .req(
            "PUT",
            "/api/v1/projects/shared/docs/x",
            &[("if-none-match", "*")],
            Some("# X\n"),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn documents_etags_and_inheritance(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (_app, router, rw, _) = setup(pool, opts).await;
    let api = Api { router, auth: rw };
    let md = [("content-type", "text/markdown")];

    // Writes need a precondition.
    let r = api
        .req("PUT", "/api/v1/projects/shared/docs/rules", &md, Some("# Rules\n"))
        .await;
    assert_eq!(r.status, StatusCode::PRECONDITION_REQUIRED);

    let r = api
        .req(
            "PUT",
            "/api/v1/projects/shared/docs/rules?message=init",
            &[("content-type", "text/markdown"), ("if-none-match", "*")],
            Some("# Rules\n\nBe terse.\n"),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let v1 = r.etag.clone().unwrap();
    assert_eq!(format!("\"{}\"", r.json()["content_hash"].as_str().unwrap()), v1);

    // Create again: precondition fails.
    let r = api
        .req(
            "PUT",
            "/api/v1/projects/shared/docs/rules",
            &[("content-type", "text/markdown"), ("if-none-match", "*")],
            Some("# Other\n"),
        )
        .await;
    assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);

    // Conditional GET.
    let r = api.req("GET", "/api/v1/projects/shared/docs/rules", &[], None).await;
    assert_eq!((r.status, r.etag.as_deref()), (StatusCode::OK, Some(v1.as_str())));
    assert_eq!(r.json()["sections"][0]["anchor"], "rules");
    let r = api
        .req(
            "GET",
            "/api/v1/projects/shared/docs/rules",
            &[("if-none-match", &v1)],
            None,
        )
        .await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::NOT_MODIFIED, ""));
    let r = api
        .req(
            "GET",
            "/api/v1/projects/shared/docs/rules",
            &[("accept", "text/markdown")],
            None,
        )
        .await;
    assert_eq!(r.body, "# Rules\n\nBe terse.\n");

    // Inherited read and root hash propagation.
    let child = api
        .req("GET", "/api/v1/projects/app/docs/rules", &[], None)
        .await
        .json();
    assert_eq!(
        (child["owner"].as_str(), child["inherited"].as_bool()),
        (Some("shared"), Some(true))
    );
    let root1 = api.req("GET", "/api/v1/projects/app/hash", &[], None).await;

    // JSON update with a stale tag, then with the current one.
    let body = r##"{"content":"# Rules\n\nBe terse and exact.\n","message":"tighten"}"##;
    let json = |tag: &'static str| [("content-type", "application/json"), ("if-match", tag)];
    let stale = Box::leak(format!("\"{}\"", "0".repeat(64)).into_boxed_str());
    let r = api
        .req("PUT", "/api/v1/projects/shared/docs/rules", &json(stale), Some(body))
        .await;
    assert_eq!(
        (r.status, r.etag.as_deref()),
        (StatusCode::PRECONDITION_FAILED, Some(v1.as_str()))
    );
    let cur = Box::leak(v1.clone().into_boxed_str());
    let r = api
        .req("PUT", "/api/v1/projects/shared/docs/rules", &json(cur), Some(body))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["changed"], true);

    let root2 = api.req("GET", "/api/v1/projects/app/hash", &[], None).await;
    assert_ne!(root1.etag, root2.etag, "parent edit changes the child's root hash");
    let r = api
        .req(
            "GET",
            "/api/v1/projects/app/tree",
            &[("if-none-match", root2.etag.as_deref().unwrap())],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);

    let hist = api
        .req("GET", "/api/v1/projects/shared/history/rules", &[], None)
        .await
        .json();
    assert_eq!(hist.as_array().unwrap().len(), 2);
    assert_eq!(hist[0]["message"], "tighten");

    // Includes expand; the resolved hash is the ETag.
    let r = api
        .req(
            "PUT",
            "/api/v1/projects/app/docs/guide",
            &[("content-type", "text/markdown"), ("if-none-match", "*")],
            Some("# Guide\n\n{{include shared:rules}}\n"),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let raw_tag = r.etag.unwrap();
    let r = api
        .req("GET", "/api/v1/projects/app/docs/guide?expand=1", &[], None)
        .await;
    let j = r.json();
    assert!(j["content"].as_str().unwrap().contains("Be terse and exact."));
    assert_eq!(j["dependencies"][0]["target"], "shared:rules");
    assert_ne!(r.etag.unwrap(), raw_tag);

    let r = api
        .req(
            "GET",
            "/api/v1/projects/app/docs/guide?section=guide&format=md",
            &[],
            None,
        )
        .await;
    assert!(r.body.starts_with("# Guide"));

    let r = api.req("GET", "/api/v1/search?q=terse", &[], None).await.json();
    assert_eq!(r[0]["path"], "rules");
    assert!(!r[0]["snippet"].as_str().unwrap().contains('\u{E000}'));

    let llms = api.req("GET", "/api/v1/projects/app/llms.txt", &[], None).await;
    assert!(llms.body.starts_with("# App\n"));
    assert!(llms.body.contains(
        "(https://docs.example/api/v1/projects/app/docs/rules?variant=human&format=md): `rules` (human, inherited from shared)"
    ));

    let r = api.req("DELETE", "/api/v1/projects/app/docs/rules", &[], None).await;
    assert_eq!(
        r.status,
        StatusCode::NOT_FOUND,
        "inherited documents cannot be deleted from a child"
    );
    let r = api.req("DELETE", "/api/v1/projects/app/docs/guide", &[], None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn sync_over_api(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (_app, router, rw, _) = setup(pool, opts).await;
    let api = Api { router, auth: rw };
    let put = |variant: &'static str, tag: String, content: &'static str, resolves: &'static str| {
        let api = &api;
        async move {
            let (name, value) = if tag == "*" {
                ("if-none-match", "*".to_owned())
            } else {
                ("if-match", tag)
            };
            api.req(
                "PUT",
                &format!("/api/v1/projects/app/docs/auth?variant={variant}&resolves={resolves}"),
                &[("content-type", "text/markdown"), (name, &value)],
                Some(content),
            )
            .await
        }
    };

    let h = put("human", "*".into(), "# Auth\n\nOpaque tokens.\n", "").await;
    assert_eq!(h.json()["sync"][0]["state"], "human_ahead");
    let q = api.req("GET", "/api/v1/projects/app/sync", &[], None).await.json();
    assert_eq!(
        (q[0]["anchor"].as_str(), q[0]["stale_side"].as_str()),
        (Some("auth"), Some("ai"))
    );

    let item = api
        .req("GET", "/api/v1/projects/app/sync/auth?anchor=auth", &[], None)
        .await
        .json();
    assert_eq!(item["human"]["body"], "# Auth\n\nOpaque tokens.\n");
    assert!(item["ai_head"].is_null());

    // Propagate to the AI variant and resolve in the same write.
    let a = put("ai", "*".into(), "# Auth\n\n- tokens: opaque\n", "auth").await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    assert_eq!(a.json()["sync"].as_array().unwrap().len(), 0);

    // Edit both sides: conflict, then resolve explicitly.
    let h2 = put("human", h.etag.unwrap(), "# Auth\n\nOpaque, rotated tokens.\n", "").await;
    put("ai", a.etag.unwrap(), "# Auth\n\n- tokens: opaque, rotated\n", "").await;
    let s = api.req("GET", "/api/v1/projects/app/sync/auth", &[], None).await.json();
    assert_eq!(s["sections"][0]["state"], "conflict");
    assert!(h2.status.is_success());

    let r = api
        .req(
            "POST",
            "/api/v1/projects/app/resolve/auth",
            &[("content-type", "application/json")],
            Some(r#"{"anchors":["auth"],"all":true}"#),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = api
        .req(
            "POST",
            "/api/v1/projects/app/resolve/auth",
            &[("content-type", "application/json")],
            Some(r#"{"paired":true}"#),
        )
        .await;
    assert_eq!(r.json()["sections"][0]["state"], "in_sync");
}

async fn mcp(r: &Router, auth: &str, body: &str) -> Reply {
    call(
        r,
        "POST",
        "/mcp",
        &[
            ("host", "localhost:3000"),
            ("authorization", auth),
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ],
        Some(body),
    )
    .await
}

/// Calls an MCP tool; returns (is_error, text of the first content block).
async fn tool(r: &Router, auth: &str, name: &str, args: Value) -> (bool, String) {
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args },
    });
    let res = mcp(r, auth, &req.to_string()).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let j = res.json();
    let result = &j["result"];
    (
        result["isError"].as_bool().unwrap_or(false),
        result["content"][0]["text"].as_str().unwrap_or_default().to_owned(),
    )
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn mcp_over_http(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (_app, router, rw, ro) = setup(pool, opts).await;
    let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;

    let r = mcp(&router, "Bearer nope", list).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
    let r = mcp(&router, &rw, init).await;
    assert_eq!(r.json()["result"]["serverInfo"]["name"], "solidate");
    let tools = mcp(&router, &rw, list).await.json();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "read_doc",
        "write_doc",
        "get_sync_queue",
        "get_sync_item",
        "resolve_sync",
        "search",
    ] {
        assert!(names.contains(&n), "missing tool {n}");
    }

    // Host validation (DNS rebinding protection).
    let r = call(
        &router,
        "POST",
        "/mcp",
        &[
            ("host", "evil.example"),
            ("authorization", &rw),
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ],
        Some(list),
    )
    .await;
    assert!(r.status.is_client_error(), "{}", r.status);

    let (err, out) = tool(
        &router,
        &rw,
        "write_doc",
        serde_json::json!({"project": "app", "path": "notes", "content": "# Notes\n\nHello.\n"}),
    )
    .await;
    assert!(!err, "{out}");
    let written: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(written["sync_pending"][0]["stale_side"], "ai");

    let (_, out) = tool(
        &router,
        &rw,
        "read_doc",
        serde_json::json!({"project": "app", "path": "notes"}),
    )
    .await;
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["content_hash"], written["content_hash"]);
    assert_eq!(doc["sections"][0]["anchor"], "notes");

    // Stale base hash is a tool error carrying the current hash.
    let (err, out) = tool(
        &router,
        &rw,
        "write_doc",
        serde_json::json!({"project": "app", "path": "notes", "content": "# Notes\n", "base_hash": "0".repeat(64)}),
    )
    .await;
    assert!(err && out.contains(written["content_hash"].as_str().unwrap()), "{out}");

    // Propagate to the AI variant.
    let (_, out) = tool(
        &router,
        &rw,
        "get_sync_item",
        serde_json::json!({"project": "app", "path": "notes", "anchor": "notes"}),
    )
    .await;
    let item: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(item["stale_side"], "ai");
    let (err, out) = tool(
        &router,
        &rw,
        "write_doc",
        serde_json::json!({"project": "app", "path": "notes", "variant": "ai", "content": "# Notes\n\n- greeting\n", "resolves": ["notes"]}),
    )
    .await;
    assert!(!err, "{out}");
    let (_, out) = tool(&router, &rw, "get_sync_queue", serde_json::json!({"project": "app"})).await;
    assert_eq!(out, "[]");

    // Restricted read-only token.
    let (err, out) = tool(
        &router,
        &ro,
        "read_doc",
        serde_json::json!({"project": "app", "path": "notes"}),
    )
    .await;
    assert!(err && out.contains("forbidden"), "{out}");
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn rate_limit_audit_and_health(pool: PgPoolOptions, opts: PgConnectOptions) {
    let config = Config {
        rate_limit: Some(solidate_app::RateLimit {
            per_minute: 1,
            burst: 3,
        }),
        ..Config::default()
    };
    let app = App::new(Db::connect_with(pool, opts).await.unwrap(), config);
    app.create_tenant(&Slug::parse("acme").unwrap(), "Acme").await.unwrap();
    let sys = app.system_ctx("acme").await.unwrap();
    let admin = app
        .create_api_token(&sys, "admin", &[Scope::Admin], None, None)
        .await
        .unwrap();
    let rw = app
        .create_api_token(&sys, "rw", &[Scope::Write], None, None)
        .await
        .unwrap();
    let router = router(app, WebConfig::default());

    let r = call(&router, "GET", "/healthz", &[], None).await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::OK, "ok"));

    let admin = Api {
        router: router.clone(),
        auth: format!("Bearer {}", admin.secret),
    };
    let log = admin.req("GET", "/api/v1/audit?limit=2", &[], None).await.json();
    let actions: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["token.create", "token.create"]);
    assert!(log["entries"][0]["at"].as_str().is_some_and(|t| t.ends_with('Z')));
    let next = log["next"].as_str().unwrap();
    let rest = admin
        .req("GET", &format!("/api/v1/audit?before={next}"), &[], None)
        .await
        .json();
    assert_eq!(rest["entries"][0]["action"], "tenant.create");
    assert!(rest["next"].is_null());

    let rw_bearer = format!("Bearer {}", rw.secret);
    let rw = Api {
        router: router.clone(),
        auth: rw_bearer.clone(),
    };
    let r = rw.req("GET", "/api/v1/audit", &[], None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    rw.req("GET", "/api/v1", &[], None).await;
    rw.req("GET", "/api/v1", &[], None).await;
    // Burst of 3 used up (including the forbidden request).
    let res = router
        .handle(
            topcoat::router::request::Request::builder()
                .uri("/api/v1")
                .header("authorization", rw_bearer.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()[header::RETRY_AFTER], "60");
    // MCP over HTTP shares the limit.
    let r = mcp(&router, &rw_bearer, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}
