//! Service-level integration tests. Require `DATABASE_URL`; see `.env.example`.

use solidate_app::core::{DocPath, Hash, Role, Scope, SectionTarget, Slug, SyncState, Variant};
use solidate_app::db::{Db, Expect};
use solidate_app::{App, AppError, Config, Ctx, PutDoc, PutResult, PutSection};
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

    // Creating the second variant translates the first: shared sections start in
    // sync, one-sided ones stay ahead.
    put(
        &app,
        &ctx,
        "app",
        "pair",
        Variant::Human,
        "# A\n\nh\n\n# Only human\n\nx\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    put(
        &app,
        &ctx,
        "app",
        "pair",
        Variant::Ai,
        "# A\n\n- a\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    let s = app.doc_sync(&ctx, "app", &path("pair")).await.unwrap();
    let states: Vec<_> = s.sections.iter().map(|x| (x.anchor.as_str(), x.state)).collect();
    assert_eq!(
        states,
        [("a", SyncState::InSync), ("only-human", SyncState::HumanAhead)]
    );
    assert_eq!(app.resolve_paired_sync(&ctx, "app", &path("pair")).await.unwrap(), 0);
    app.set_doc_sync(&ctx, "app", &path("pair"), false).await.unwrap();

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
async fn section_writes(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let human = "# Auth\n\nTokens are opaque.\n\n## Expiry\n\nSessions last 14 days.\n";
    let ai = "# Auth\n\n- tokens: opaque\n\n## Expiry\n\n- session_ttl: 14d\n";
    put(&app, &ctx, "shared", "auth", Variant::Human, human, Expect::Absent, &[])
        .await
        .unwrap();
    put(&app, &ctx, "shared", "auth", Variant::Ai, ai, Expect::Absent, &[])
        .await
        .unwrap();
    let section = |variant, target, content, expect, section_hash, resolves: &'static [&'static str]| {
        let (app, ctx) = (&app, &ctx);
        async move {
            let resolves: Vec<String> = resolves.iter().map(|s| s.to_string()).collect();
            app.put_section(
                ctx,
                PutSection {
                    project: "shared",
                    path: &path("auth"),
                    variant,
                    target,
                    content,
                    expect,
                    section_hash,
                    message: None,
                    resolves: &resolves,
                },
            )
            .await
        }
    };
    let expiry = SectionTarget::Section {
        anchor: "expiry",
        subsections: false,
    };
    let old = solidate_app::core::markdown::semantic_hash("## Expiry\n\nSessions last 14 days.\n");

    let r = section(
        Variant::Human,
        expiry,
        "## Expiry\n\nSessions last 30 days.",
        Expect::Any,
        Some(old),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(r.anchors, ["expiry"]);
    let states: Vec<_> = r.put.sync.iter().map(|s| (s.anchor.as_str(), s.state)).collect();
    assert_eq!(states, [("auth", SyncState::InSync), ("expiry", SyncState::HumanAhead)]);
    let v = app
        .get_doc(&ctx, "shared", &path("auth"), Variant::Human)
        .await
        .unwrap();
    assert_eq!(v.head.unwrap().content, human.replace("14 days", "30 days"));

    // The old section hash no longer matches.
    assert!(matches!(
        section(Variant::Human, expiry, "## Expiry\n\nx", Expect::Any, Some(old), &[]).await,
        Err(AppError::PreconditionFailed { current: Some(_) })
    ));

    // Translating one section and resolving it clears the queue.
    section(
        Variant::Ai,
        expiry,
        "## Expiry\n\n- session_ttl: 30d\n",
        Expect::Head(Hash::of(ai)),
        None,
        &["expiry"],
    )
    .await
    .unwrap();
    assert!(app.sync_queue(&ctx, "shared").await.unwrap().is_empty());

    // Missing variants can only be created by appending; inherited documents are
    // overridden in the writing project.
    let r = app
        .put_section(
            &ctx,
            PutSection {
                project: "app",
                path: &path("auth"),
                variant: Variant::Human,
                target: SectionTarget::After("auth"),
                content: "# Local\n\nApp only.\n",
                expect: Expect::Any,
                section_hash: None,
                message: None,
                resolves: &[],
            },
        )
        .await
        .unwrap();
    assert_eq!(r.anchors, ["local"]);
    let v = app.get_doc(&ctx, "app", &path("auth"), Variant::Human).await.unwrap();
    assert!(!v.inherited() && v.head.unwrap().content.ends_with("30 days.\n\n# Local\n\nApp only.\n"));
    assert!(matches!(
        section(
            Variant::Human,
            SectionTarget::After("nope"),
            "# X\n",
            Expect::Any,
            None,
            &[]
        )
        .await,
        Err(AppError::Invalid(_))
    ));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn translation_proposals(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let human = "# Auth\n\nTokens are opaque.\n\n## Expiry\n\nSessions last 14 days.\n";
    let ai = "# Auth\n\n- tokens: opaque\n\n## Expiry\n\n- session_ttl: 14d\n";
    put(&app, &ctx, "app", "auth", Variant::Human, human, Expect::Absent, &[])
        .await
        .unwrap();
    put(&app, &ctx, "app", "auth", Variant::Ai, ai, Expect::Absent, &[])
        .await
        .unwrap();
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());

    let guide = app.translation_guide(&ctx, "app").await.unwrap();
    assert_eq!(guide.source, None);
    assert_eq!(guide.content, solidate_app::DEFAULT_GUIDE);

    let human2 = human.replace("14 days", "30 days");
    put(&app, &ctx, "app", "auth", Variant::Human, &human2, Expect::Any, &[])
        .await
        .unwrap();
    let propose = |content: &'static str, base: Option<Hash>, resolves: Vec<String>| {
        let app = app.clone();
        let ctx = ctx.clone();
        async move {
            app.propose(
                &ctx,
                solidate_app::Propose {
                    project: "app",
                    path: &path("auth"),
                    variant: Variant::Ai,
                    content,
                    base,
                    message: Some("ttl 30d"),
                    resolves: &resolves,
                },
            )
            .await
        }
    };
    let ai2 = "# Auth\n\n- tokens: opaque\n\n## Expiry\n\n- session_ttl: 30d\n";

    // Stale base and unknown anchors are refused.
    assert!(matches!(
        propose(ai2, None, vec![]).await,
        Err(AppError::PreconditionFailed { current: Some(h) }) if h == Hash::of(ai)
    ));
    assert!(matches!(
        propose(ai2, Some(Hash::of(ai)), vec!["nope".into()]).await,
        Err(AppError::Invalid(_))
    ));

    // Default resolves: every pending section.
    let p = propose(ai2, Some(Hash::of(ai)), vec![]).await.unwrap();
    assert_eq!(p.proposal.resolves, ["expiry"]);
    assert!(!p.outdated);
    assert!(p.diff.contains("+- session_ttl: 30d"), "{}", p.diff);
    let q = app.sync_queue(&ctx, "app").await.unwrap();
    assert_eq!(q[0].proposal.map(|r| (r.id, r.outdated)), Some((p.proposal.id, false)));

    // A new submission replaces the open one.
    let p = propose(ai2, Some(Hash::of(ai)), vec!["expiry".into()]).await.unwrap();
    assert_eq!(app.project_proposals(&ctx, "app").await.unwrap().len(), 1);

    // Accepting writes the variant, resolves the section and removes the proposal.
    let r = app.accept_proposal(&ctx, "app", p.proposal.id).await.unwrap();
    assert_eq!(r.revision.content_hash, Hash::of(ai2));
    assert_eq!(r.revision.message.as_deref(), Some("ttl 30d"));
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());
    assert!(app.project_proposals(&ctx, "app").await.unwrap().is_empty());
    assert!(matches!(
        app.accept_proposal(&ctx, "app", p.proposal.id).await,
        Err(AppError::NotFound)
    ));

    // The source moving after submission makes a proposal outdated.
    let human3 = human2.replace("opaque", "random");
    put(&app, &ctx, "app", "auth", Variant::Human, &human3, Expect::Any, &[])
        .await
        .unwrap();
    let ai3 = "# Auth\n\n- tokens: random\n\n## Expiry\n\n- session_ttl: 30d\n";
    let p = propose(ai3, Some(Hash::of(ai2)), vec![]).await.unwrap();
    let human4 = human3.replace("30 days", "31 days");
    put(&app, &ctx, "app", "auth", Variant::Human, &human4, Expect::Any, &[])
        .await
        .unwrap();
    assert!(app.doc_proposals(&ctx, "app", &path("auth")).await.unwrap()[0].outdated);
    assert!(matches!(
        app.accept_proposal(&ctx, "app", p.proposal.id).await,
        Err(AppError::PreconditionFailed { .. })
    ));
    assert_eq!(app.reject_proposal(&ctx, "app", p.proposal.id).await.unwrap(), "auth");
    assert!(app.project_proposals(&ctx, "app").await.unwrap().is_empty());

    // Writing the target variant directly supersedes its proposal.
    propose(ai3, Some(Hash::of(ai2)), vec![]).await.unwrap();
    put(&app, &ctx, "app", "auth", Variant::Ai, ai3, Expect::Any, &[])
        .await
        .unwrap();
    assert!(app.project_proposals(&ctx, "app").await.unwrap().is_empty());

    // A guide in a parent project applies through inheritance.
    put(
        &app,
        &ctx,
        "shared",
        solidate_app::GUIDE_PATH,
        Variant::Human,
        "# Guide\n\nAI variant: YAML-like lists.\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    let guide = app.translation_guide(&ctx, "app").await.unwrap();
    assert_eq!(guide.source.as_deref(), Some("shared:_meta/translation"));
    assert!(guide.content.contains("YAML-like"));
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

    // Users without a password cannot sign in with one until it is set.
    let sso = app.db().create_user("sso@acme.dev", "SSO", None).await.unwrap();
    assert!(matches!(
        app.login("sso@acme.dev", "long-enough-pw").await,
        Err(AppError::Unauthorized)
    ));
    app.set_password(sso.id, "long-enough-pw").await.unwrap();
    assert_eq!(app.login("sso@acme.dev", "long-enough-pw").await.unwrap().id, sso.id);
    app.set_password(sso.id, "another-long-pw").await.unwrap();
    assert!(matches!(
        app.login("sso@acme.dev", "long-enough-pw").await,
        Err(AppError::Unauthorized)
    ));

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

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn audit_log_records_mutations(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    put(&app, &ctx, "app", "a", Variant::Human, "# A\n", Expect::Absent, &[])
        .await
        .unwrap();
    // Unchanged content writes no entry.
    put(&app, &ctx, "app", "a", Variant::Human, "# A\n", Expect::Any, &[])
        .await
        .unwrap();
    let rw = app
        .create_api_token(&ctx, "rw", &[Scope::Write], None, None)
        .await
        .unwrap();
    app.delete_doc(&ctx, "app", &path("a")).await.unwrap();

    let log = app.audit_log(&ctx, 100, None).await.unwrap();
    let actions: Vec<&str> = log.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "doc.delete",
            "token.create",
            "doc.write",
            "project.create",
            "project.create",
            "tenant.create"
        ]
    );
    let write = &log[2];
    assert_eq!(
        (
            write.actor_kind.as_str(),
            write.project_slug.as_deref(),
            write.target.as_deref()
        ),
        ("system", Some("app"), Some("a"))
    );
    assert_eq!(write.detail["variant"], "human");
    assert_eq!(log[1].target.as_deref(), Some(rw.token.id.to_string().as_str()));
    assert!(log[1].detail.get("secret").is_none());

    let page = app.audit_log(&ctx, 2, Some(log[1].id)).await.unwrap();
    assert_eq!(page.iter().map(|e| e.id).collect::<Vec<_>>(), [log[2].id, log[3].id]);

    // Token writes are attributed to the token; reading the log needs admin.
    let tctx = app.token_ctx(&rw.secret).await.unwrap();
    put(&app, &tctx, "app", "b", Variant::Human, "# B\n", Expect::Absent, &[])
        .await
        .unwrap();
    let latest = &app.audit_log(&ctx, 1, None).await.unwrap()[0];
    assert_eq!(
        (latest.action.as_str(), latest.actor_token_id),
        ("doc.write", Some(rw.token.id))
    );
    assert!(matches!(app.audit_log(&tctx, 10, None).await, Err(AppError::Forbidden)));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn tokens_are_rate_limited(pool: PgPoolOptions, opts: PgConnectOptions) {
    let config = Config {
        rate_limit: Some(solidate_app::RateLimit {
            per_minute: 1,
            burst: 2,
        }),
        ..Config::default()
    };
    let app = App::new(Db::connect_with(pool, opts).await.unwrap(), config);
    app.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let ctx = app.system_ctx("acme").await.unwrap();
    let a = app
        .create_api_token(&ctx, "a", &[Scope::Read], None, None)
        .await
        .unwrap();
    let b = app
        .create_api_token(&ctx, "b", &[Scope::Read], None, None)
        .await
        .unwrap();

    app.token_ctx(&a.secret).await.unwrap();
    app.token_ctx(&a.secret).await.unwrap();
    assert!(matches!(
        app.token_ctx(&a.secret).await,
        Err(AppError::RateLimited {
            retry_after_secs: 59..=60
        })
    ));
    app.token_ctx(&b.secret).await.unwrap();
    // Invalid secrets are rejected before counting against the token.
    let wrong = format!("{}_{}", a.token.prefix, "0".repeat(64));
    assert!(matches!(app.token_ctx(&wrong).await, Err(AppError::Unauthorized)));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn source_drift_flow(pool: PgPoolOptions, opts: PgConnectOptions) {
    use solidate_app::SourceReport;
    use solidate_app::core::sources::{DriftState, Files};

    let (app, ctx) = setup(pool, opts).await;
    let files = |e: &[(&str, &str)]| -> Files { e.iter().map(|(p, h)| (p.to_string(), h.to_string())).collect() };
    let report = |f: Files, removed: Vec<String>, replace: bool, rev: &'static str| {
        let (app, ctx) = (&app, &ctx);
        async move {
            app.report_sources(
                ctx,
                "app",
                SourceReport {
                    revision: Some(rev),
                    files: &f,
                    removed: &removed,
                    replace,
                },
            )
            .await
        }
    };
    let queue = || async { app.drift_queue(&ctx, "app").await.unwrap() };
    let verify = |anchors: &[&str], rev: Option<&str>| {
        let anchors: Vec<String> = anchors.iter().map(|s| s.to_string()).collect();
        let rev = rev.map(str::to_owned);
        let (app, ctx) = (&app, &ctx);
        async move {
            app.verify_sources(ctx, "app", &path("design/auth"), &anchors, rev.as_deref())
                .await
        }
    };

    let human = "# Auth {#auth}\n\nOpaque tokens.\n\n<!-- sources: src/auth.rs, src/db/ -->\n\n# Other\n\nx.\n";
    let ai = "# Auth {#auth}\n\nOpaque tokens.\n\n# Other\n\nx.\n";
    let h = put(
        &app,
        &ctx,
        "app",
        "design/auth",
        Variant::Human,
        human,
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    put(&app, &ctx, "app", "design/auth", Variant::Ai, ai, Expect::Absent, &[])
        .await
        .unwrap();
    assert!(
        app.sync_queue(&ctx, "app").await.unwrap().is_empty(),
        "directives are not content"
    );

    // Adding the binding to the AI variant too does not affect sync.
    let a = put(
        &app,
        &ctx,
        "app",
        "design/auth",
        Variant::Ai,
        "# Auth {#auth}\n\n<!-- sources: src/auth.rs -->\n\nOpaque tokens.\n\n# Other\n\nx.\n",
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    assert!(a.sync.iter().all(|s| s.state == SyncState::InSync));

    // Nothing reported: unverified, every pattern missing.
    let q = queue().await;
    assert_eq!((q.revision.as_deref(), q.reported_at), (None, None));
    assert_eq!(q.entries.len(), 1);
    let e = &q.entries[0];
    assert_eq!((e.anchor.as_str(), e.state), ("auth", DriftState::Unverified));
    assert_eq!(e.section_title, "Auth");
    assert_eq!(e.patterns, ["src/auth.rs", "src/db/"]);
    assert_eq!(e.missing, ["src/auth.rs", "src/db/"]);
    assert!(matches!(verify(&["auth"], None).await, Err(AppError::Invalid(_))));

    // Reverse lookup needs no report.
    let hit = app
        .affected_sections(&ctx, "app", &["src/db/x.sql".into(), "README.md".into()])
        .await
        .unwrap();
    assert_eq!(hit.len(), 1);
    assert_eq!(
        (hit[0].anchor.as_str(), hit[0].paths.clone()),
        ("auth", vec!["src/db/x.sql".to_owned()])
    );

    let s = report(
        files(&[("src/auth.rs", "a1"), ("src/db/x.sql", "x1"), ("README.md", "r1")]),
        vec![],
        true,
        "r1",
    )
    .await
    .unwrap();
    assert_eq!((s.files, s.revision.as_deref()), (3, Some("r1")));
    let q = queue().await;
    assert_eq!(q.revision.as_deref(), Some("r1"));
    assert_eq!(q.entries[0].state, DriftState::Unverified);
    assert!(q.entries[0].missing.is_empty());

    assert!(matches!(verify(&["auth"], Some("r0")).await, Err(AppError::Invalid(_))));
    assert!(matches!(verify(&["other"], None).await, Err(AppError::Invalid(_))));
    let d = verify(&["auth"], Some("r1")).await.unwrap();
    assert_eq!(d[0].state, DriftState::Fresh);
    assert_eq!(d[0].verified_revision.as_deref(), Some("r1"));
    assert!(queue().await.entries.is_empty());

    // An unrelated file changing leaves the section fresh.
    report(files(&[("README.md", "r2")]), vec![], false, "r2")
        .await
        .unwrap();
    assert!(queue().await.entries.is_empty());

    report(
        files(&[("src/auth.rs", "a2"), ("src/db/y.sql", "y1")]),
        vec![],
        false,
        "r3",
    )
    .await
    .unwrap();
    let e = queue().await.entries.remove(0);
    assert_eq!(e.state, DriftState::Changed);
    assert_eq!(e.delta.changed, ["src/auth.rs"]);
    assert_eq!(e.delta.added, ["src/db/y.sql"]);
    assert!(e.delta.removed.is_empty());
    assert_eq!(e.verified_revision.as_deref(), Some("r1"));

    let s = report(Files::new(), vec!["src/auth.rs".into()], false, "r4")
        .await
        .unwrap();
    assert_eq!(s.files, 3);
    let e = queue().await.entries.remove(0);
    assert_eq!(e.delta.removed, ["src/auth.rs"]);
    assert_eq!(e.missing, ["src/auth.rs"]);

    // Verified with a missing pattern: fresh, but still listed until the pattern is fixed.
    let d = verify(&["auth"], None).await.unwrap();
    assert_eq!(d[0].state, DriftState::Fresh);
    assert_eq!(queue().await.entries.len(), 1);

    // Invalid inputs.
    let bad = put(
        &app,
        &ctx,
        "app",
        "design/bad",
        Variant::Human,
        "# X\n\n<!-- sources: ../etc/passwd -->\n",
        Expect::Absent,
        &[],
    )
    .await;
    assert!(matches!(bad, Err(AppError::Invalid(_))), "{bad:?}");
    let bad = report(files(&[("/abs", "h")]), vec![], false, "r5").await;
    assert!(matches!(bad, Err(AppError::Invalid(_))));
    let bad = report(Files::new(), vec!["x".into()], true, "r5").await;
    assert!(matches!(bad, Err(AppError::Invalid(_))));

    // Dropping the bindings from both variants drops the verification record.
    put(
        &app,
        &ctx,
        "app",
        "design/auth",
        Variant::Human,
        ai,
        Expect::Head(h.revision.content_hash),
        &[],
    )
    .await
    .unwrap();
    put(
        &app,
        &ctx,
        "app",
        "design/auth",
        Variant::Ai,
        ai,
        Expect::Head(a.revision.content_hash),
        &[],
    )
    .await
    .unwrap();
    assert!(queue().await.entries.is_empty());
    put(
        &app,
        &ctx,
        "app",
        "design/auth",
        Variant::Human,
        human,
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(queue().await.entries[0].state, DriftState::Unverified);

    // Read-only callers cannot report or verify.
    let ro = app
        .create_api_token(&ctx, "ro", &[Scope::Read], None, None)
        .await
        .unwrap();
    let ro = app
        .authenticate(solidate_app::Credential::Bearer(&ro.secret))
        .await
        .unwrap();
    let denied = app
        .report_sources(
            &ro,
            "app",
            SourceReport {
                revision: None,
                files: &Files::new(),
                removed: &[],
                replace: false,
            },
        )
        .await;
    assert!(matches!(denied, Err(AppError::Forbidden)));
    assert!(app.drift_queue(&ro, "app").await.is_ok());
}
