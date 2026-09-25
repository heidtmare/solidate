//! Service-level integration tests. Require `DATABASE_URL`; see `.env.example`.

use solidate_app::core::{DocPath, Hash, Role, Scope, Slug, SyncState, Variant};
use solidate_app::db::{Db, Expect};
use solidate_app::{App, AppError, Config, Ctx, PutDoc, PutResult};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

async fn setup(pool: PgPoolOptions, opts: PgConnectOptions) -> (App, Ctx) {
    let app = App::new(Db::connect_with(pool, opts).await.unwrap(), Config::default());
    app.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let ctx = app.system_ctx("acme").await.unwrap();
    app.create_project(&ctx, &slug("shared"), "Shared", None).await.unwrap();
    app.create_project(&ctx, &slug("app"), "App", Some("shared"))
        .await
        .unwrap();
    (app, ctx)
}

fn slug(s: &str) -> Slug {
    Slug::parse(s).unwrap()
}

fn path(s: &str) -> DocPath {
    DocPath::parse(s).unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn put(
    app: &App,
    ctx: &Ctx,
    project: &str,
    p: &str,
    variant: Variant,
    content: &str,
    expect: Expect,
    resolves: &[&str],
) -> Result<PutResult, AppError> {
    let resolves: Vec<String> = resolves.iter().map(|s| s.to_string()).collect();
    app.put_doc(
        ctx,
        PutDoc {
            project,
            path: &path(p),
            variant,
            content,
            expect,
            message: None,
            resolves: &resolves,
        },
    )
    .await
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn inheritance_override_and_tree_hash(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let rules = "# Style\n\nUse plain language.\n";
    put(
        &app,
        &ctx,
        "shared",
        "rules/style",
        Variant::Human,
        rules,
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();

    let v = app
        .get_doc(&ctx, "app", &path("rules/style"), Variant::Human)
        .await
        .unwrap();
    assert!(v.inherited());
    assert_eq!(v.owner.slug, "shared");

    let before = app.tree(&ctx, "app").await.unwrap();
    assert_eq!(before.entries.len(), 1);
    assert!(before.entries[0].inherited);

    // Parent change propagates to the child's root hash.
    put(
        &app,
        &ctx,
        "shared",
        "rules/style",
        Variant::Human,
        "# Style\n\nBe brief.\n",
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    let after = app.tree(&ctx, "app").await.unwrap();
    assert_ne!(before.root_hash, after.root_hash);

    // Override from the child with If-Match on the inherited head.
    let inherited = Hash::of("# Style\n\nBe brief.\n");
    put(
        &app,
        &ctx,
        "app",
        "rules/style",
        Variant::Human,
        "# Style\n\nApp rules.\n",
        Expect::Head(inherited),
        &[],
    )
    .await
    .unwrap();
    let v = app
        .get_doc(&ctx, "app", &path("rules/style"), Variant::Human)
        .await
        .unwrap();
    assert!(!v.inherited());
    assert!(matches!(
        put(
            &app,
            &ctx,
            "app",
            "rules/style",
            Variant::Human,
            "x",
            Expect::Head(inherited),
            &[]
        )
        .await,
        Err(AppError::PreconditionFailed { .. })
    ));

    // Deleting the override re-exposes the inherited document.
    app.delete_doc(&ctx, "app", &path("rules/style")).await.unwrap();
    assert!(
        app.get_doc(&ctx, "app", &path("rules/style"), Variant::Human)
            .await
            .unwrap()
            .inherited()
    );
    assert!(matches!(
        app.delete_doc(&ctx, "app", &path("rules/style")).await,
        Err(AppError::NotFound)
    ));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn sync_flow(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let human = "# Auth\n\nTokens are opaque.\n\n## Expiry\n\nSessions last 14 days.\n";
    let r = put(&app, &ctx, "app", "auth", Variant::Human, human, Expect::Absent, &[])
        .await
        .unwrap();
    let states: Vec<_> = r.sync.iter().map(|s| (s.anchor.as_str(), s.state)).collect();
    assert_eq!(
        states,
        [("auth", SyncState::HumanAhead), ("expiry", SyncState::HumanAhead)]
    );
    assert_eq!(app.sync_queue(&ctx, "app").await.unwrap().len(), 2);

    // Agent writes the AI variant and resolves both sections.
    let ai = "# Auth\n\n- tokens: opaque\n\n## Expiry\n\n- session_ttl: 14d\n";
    let r = put(
        &app,
        &ctx,
        "app",
        "auth",
        Variant::Ai,
        ai,
        Expect::Absent,
        &["auth", "expiry"],
    )
    .await
    .unwrap();
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());

    // Human edits one section: AI side is stale; detail carries base and diff.
    let human2 = human.replace("14 days", "30 days");
    put(
        &app,
        &ctx,
        "app",
        "auth",
        Variant::Human,
        &human2,
        Expect::Head(Hash::of(human)),
        &[],
    )
    .await
    .unwrap();
    let q = app.sync_queue(&ctx, "app").await.unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(
        (q[0].anchor.as_str(), q[0].state, q[0].stale_side),
        ("expiry", SyncState::HumanAhead, Some(Variant::Ai))
    );
    let item = app.sync_item(&ctx, "app", &path("auth"), "expiry").await.unwrap();
    assert!(item.human_base.as_deref().unwrap().contains("14 days"));
    let diff = item.human_diff.unwrap();
    assert!(
        diff.contains("-Sessions last 14 days.") && diff.contains("+Sessions last 30 days."),
        "{diff}"
    );
    assert_eq!(item.ai_head, Some(Hash::of(ai)));

    // Both sides edited since the last sync: conflict.
    let ai2 = ai.replace("14d", "31d");
    put(&app, &ctx, "app", "auth", Variant::Ai, &ai2, Expect::Any, &[])
        .await
        .unwrap();
    let s = app.doc_sync(&ctx, "app", &path("auth")).await.unwrap();
    assert_eq!(
        s.sections.iter().find(|x| x.anchor == "expiry").unwrap().state,
        SyncState::Conflict
    );

    // Resolve without editing.
    let s = app
        .resolve_sync(&ctx, "app", &path("auth"), &["expiry".into()])
        .await
        .unwrap();
    assert!(s.sections.iter().all(|x| x.state == SyncState::InSync));
    assert!(matches!(
        app.resolve_sync(&ctx, "app", &path("auth"), &["nope".into()]).await,
        Err(AppError::Invalid(_))
    ));

    // Sync disabled: excluded from queue.
    put(
        &app,
        &ctx,
        "app",
        "notes",
        Variant::Human,
        "# Notes\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    app.set_doc_sync(&ctx, "app", &path("notes"), false).await.unwrap();
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn render_with_includes_and_links(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    put(
        &app,
        &ctx,
        "shared",
        "rules",
        Variant::Human,
        "# Rules\n\n## Tone {#tone}\n\nBe kind.\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    let doc = "# Guide\n\n{{include shared:rules#tone}}\n\nSee [[design/auth]].\n";
    put(&app, &ctx, "app", "guide", Variant::Human, doc, Expect::Absent, &[])
        .await
        .unwrap();

    let href = |t: &solidate_app::core::LinkTarget| Some(format!("/x/{t}"));
    let r1 = app
        .render_doc(&ctx, "app", &path("guide"), Variant::Human, &href)
        .await
        .unwrap();
    assert!(r1.html.contains("Be kind."), "{}", r1.html);
    assert!(r1.html.contains(r#"href="/x/design/auth""#));
    assert_eq!(r1.dependencies.len(), 1);

    put(
        &app,
        &ctx,
        "shared",
        "rules",
        Variant::Human,
        "# Rules\n\n## Tone {#tone}\n\nBe direct.\n",
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    let r2 = app
        .render_doc(&ctx, "app", &path("guide"), Variant::Human, &href)
        .await
        .unwrap();
    assert!(r2.html.contains("Be direct."));
    assert_ne!(
        r1.resolved_hash, r2.resolved_hash,
        "include change invalidates dependent"
    );

    // AI variant falls back to the human variant of included docs.
    put(
        &app,
        &ctx,
        "app",
        "guide",
        Variant::Ai,
        "{{include shared:rules#tone}}\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    let r3 = app
        .render_doc(&ctx, "app", &path("guide"), Variant::Ai, &href)
        .await
        .unwrap();
    assert!(r3.html.contains("Be direct."));

    let (_, revs) = app
        .history(&ctx, "shared", &path("rules"), Variant::Human, 10)
        .await
        .unwrap();
    assert_eq!(revs.len(), 2);
    let d = app
        .revision_diff(&ctx, "shared", &path("rules"), revs[0].id)
        .await
        .unwrap();
    assert!(d.diff.contains("-Be kind.") && d.diff.contains("+Be direct."));

    let hits = app.search(&ctx, "direct", None, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn users_tokens_and_authorization(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, admin) = setup(pool, opts).await;
    assert!(matches!(
        app.register_user("a@b.c", "A", "short").await,
        Err(AppError::Invalid(_))
    ));
    let user = app
        .register_user("reader@acme.dev", "Reader", "long-enough-pw")
        .await
        .unwrap();
    assert!(matches!(
        app.login("reader@acme.dev", "wrong-password").await,
        Err(AppError::Unauthorized)
    ));
    assert!(matches!(
        app.login("nobody@acme.dev", "whatever-pw").await,
        Err(AppError::Unauthorized)
    ));
    assert_eq!(
        app.login("READER@acme.dev", "long-enough-pw").await.unwrap().id,
        user.id
    );

    assert!(matches!(app.user_ctx("acme", user.id).await, Err(AppError::Forbidden)));
    app.set_member(&admin, user.id, Role::Reader).await.unwrap();
    let reader = app.user_ctx("acme", user.id).await.unwrap();
    assert!(matches!(
        put(&app, &reader, "app", "x", Variant::Human, "# x\n", Expect::Any, &[]).await,
        Err(AppError::Forbidden)
    ));

    // Token restricted to `app` with write scope.
    let app_project = app.project(&admin, "app").await.unwrap();
    let t = app
        .create_api_token(&admin, "ci", &[Scope::Write], Some(app_project.id), None)
        .await
        .unwrap();
    assert!(t.secret.starts_with("sol_"));
    let tctx = app.token_ctx(&t.secret).await.unwrap();
    put(&app, &tctx, "app", "ci", Variant::Human, "# CI\n", Expect::Absent, &[])
        .await
        .unwrap();
    assert!(matches!(
        put(
            &app,
            &tctx,
            "shared",
            "ci",
            Variant::Human,
            "# CI\n",
            Expect::Absent,
            &[]
        )
        .await,
        Err(AppError::Forbidden)
    ));
    assert_eq!(app.projects(&tctx).await.unwrap().len(), 1);
    assert!(matches!(app.api_tokens(&tctx).await, Err(AppError::Forbidden)));

    // Tampered and revoked tokens fail.
    let last = if t.secret.ends_with('0') { '1' } else { '0' };
    let tampered = format!("{}{last}", &t.secret[..t.secret.len() - 1]);
    assert!(matches!(app.token_ctx(&tampered).await, Err(AppError::Unauthorized)));
    app.revoke_api_token(&admin, t.token.id).await.unwrap();
    assert!(matches!(app.token_ctx(&t.secret).await, Err(AppError::Unauthorized)));
}
