//! Service-level integration tests. Require `DATABASE_URL`; see `.env.example`.

use solidate_app::core::{DocPath, Hash, Role, Scope, SectionTarget, Slug, SyncState, Variant};
use solidate_app::db::{Db, Expect, RevisionId};
use solidate_app::{App, AppError, Changes, Config, Ctx, PutDoc, PutResult, PutSection, Restore, RestoreResult};
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
    // One variant only: nothing to sync yet, listed as untranslated.
    assert!(r.sync.is_empty());
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());
    let u = app.untranslated(&ctx, "app").await.unwrap();
    assert_eq!(
        u.iter().map(|x| (x.path.as_str(), x.missing)).collect::<Vec<_>>(),
        [("auth", Variant::Ai)]
    );

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
    assert!(app.untranslated(&ctx, "app").await.unwrap().is_empty());

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
    assert_eq!(app.untranslated(&ctx, "app").await.unwrap().len(), 1);
    app.set_doc_sync(&ctx, "app", &path("notes"), false).await.unwrap();
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());
    assert!(app.untranslated(&ctx, "app").await.unwrap().is_empty());
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn diagram_edits_follow_without_sync_debt(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let fence = |edge: &str| format!("```mermaid\ngraph TD\n  {edge}\n```\n");
    let human = format!("# Flow\n\nClient calls API.\n\n{}", fence("C-->A"));
    let ai = format!("# Flow\n\n- client -> api\n\n{}", fence("C-->A"));
    put(&app, &ctx, "app", "flow", Variant::Human, &human, Expect::Absent, &[])
        .await
        .unwrap();
    put(&app, &ctx, "app", "flow", Variant::Ai, &ai, Expect::Absent, &[])
        .await
        .unwrap();

    // Editing the diagram leaves sync alone and updates the identical AI copy.
    let human2 = human.replace("C-->A", "C-->G-->A");
    let r = put(
        &app,
        &ctx,
        "app",
        "flow",
        Variant::Human,
        &human2,
        Expect::Head(Hash::of(&human)),
        &[],
    )
    .await
    .unwrap();
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync), "{:?}", r.sync);
    let ai2 = ai.replace("C-->A", "C-->G-->A");
    assert_eq!(r.followed, Some(Hash::of(&ai2)));
    assert_eq!(
        (r.diagrams_carried, r.diagrams_skipped),
        (vec!["flow".to_owned()], vec![])
    );
    let head = app
        .get_doc(&ctx, "app", &path("flow"), Variant::Ai)
        .await
        .unwrap()
        .head
        .unwrap();
    assert_eq!(head.content, ai2);
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());

    // A diverged AI copy is left alone.
    put(
        &app,
        &ctx,
        "app",
        "flow",
        Variant::Ai,
        &ai2.replace("G-->A", "G-->X"),
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    let r = put(
        &app,
        &ctx,
        "app",
        "flow",
        Variant::Human,
        &human2.replace("G-->A", "G-->B"),
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(r.followed, None);
    assert_eq!(
        (r.diagrams_carried, r.diagrams_skipped),
        (vec![], vec!["flow".to_owned()])
    );
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());

    // The diverged copy is reported as drift, and copying the diagram clears it
    // without touching sync.
    let s = app.doc_sync(&ctx, "app", &path("flow")).await.unwrap();
    assert_eq!(
        s.diagram_drift
            .iter()
            .map(|d| (d.anchor.as_str(), d.index))
            .collect::<Vec<_>>(),
        [("flow", 0)]
    );
    let listed = app.diagram_drift(&ctx, "app").await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|d| (d.path.as_str(), d.anchor.as_str()))
            .collect::<Vec<_>>(),
        [("flow", "flow")]
    );
    let hash = app
        .copy_diagram(&ctx, "app", &path("flow"), "flow", 0, Variant::Human)
        .await
        .unwrap();
    let head = app
        .get_doc(&ctx, "app", &path("flow"), Variant::Ai)
        .await
        .unwrap()
        .head
        .unwrap();
    assert_eq!(head.content_hash, hash);
    assert_eq!(head.content, ai2.replace("G-->A", "G-->B"));
    assert!(
        app.doc_sync(&ctx, "app", &path("flow"))
            .await
            .unwrap()
            .diagram_drift
            .is_empty()
    );
    assert!(app.sync_queue(&ctx, "app").await.unwrap().is_empty());
    assert!(matches!(
        app.copy_diagram(&ctx, "app", &path("flow"), "flow", 0, Variant::Human)
            .await,
        Err(AppError::Invalid(_))
    ));

    // Adding a diagram is a change the AI variant must pick up.
    let human3 = format!("{}\n{}", human2.replace("G-->A", "G-->B"), fence("X-->Y"));
    let r = put(&app, &ctx, "app", "flow", Variant::Human, &human3, Expect::Any, &[])
        .await
        .unwrap();
    assert_eq!(r.followed, None);
    let q = app.sync_queue(&ctx, "app").await.unwrap();
    assert_eq!(
        q.iter().map(|e| (e.anchor.as_str(), e.state)).collect::<Vec<_>>(),
        [("flow", SyncState::HumanAhead)]
    );
    assert!(!q[0].state.needs_person());
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

    let hits = app.search(&ctx, "direct", None, None, 10).await.unwrap();
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

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn search_returns_matching_sections(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let human = "# Auth\n\nOverview.\n\n## Tokens\n\nTokens are rotated daily.\n\n## Sessions\n\nSessions expire; tokens are rotated on logout.\n\n## Other\n\nNothing here.\n";
    let ai = "# Auth\n\n## Tokens\n\n- rotated daily\n";
    put(
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

    let hits = app
        .search(&ctx, "rotated", None, Some(Variant::Human), 10)
        .await
        .unwrap();
    let mut anchors: Vec<_> = hits.iter().map(|h| h.anchor.as_deref().unwrap()).collect();
    anchors.sort();
    assert_eq!(anchors, ["sessions", "tokens"]);
    let tokens = hits.iter().find(|h| h.anchor.as_deref() == Some("tokens")).unwrap();
    assert_eq!(tokens.section_title.as_deref(), Some("Tokens"));
    // The hit's hash is accepted as a section write precondition.
    app.put_section(
        &ctx,
        PutSection {
            project: "app",
            path: &path("design/auth"),
            variant: Variant::Human,
            target: SectionTarget::Section {
                anchor: "tokens",
                subsections: false,
            },
            content: "## Tokens\n\nTokens are rotated hourly.\n",
            expect: Expect::Any,
            section_hash: tokens.section_hash,
            message: None,
            resolves: &[],
        },
    )
    .await
    .unwrap();

    let ai_hits = app.search(&ctx, "rotated", None, Some(Variant::Ai), 10).await.unwrap();
    assert_eq!(
        ai_hits
            .iter()
            .map(|h| (h.variant, h.anchor.as_deref()))
            .collect::<Vec<_>>(),
        [(Variant::Ai, Some("tokens"))]
    );
    assert_eq!(app.search(&ctx, "rotated", None, None, 1).await.unwrap().len(), 1);

    // Path-only match: no section anchor.
    let by_path = app.search(&ctx, "design", None, None, 10).await.unwrap();
    assert!(!by_path.is_empty() && by_path.iter().all(|h| h.anchor.is_none()));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn section_context_prefers_variant_and_lists_links(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let human = "# Auth\n\n## Tokens\n\n<!-- sources: src/auth/ -->\n\nTokens are opaque. See [[decisions/0001]].\n\n## Legacy\n\n<!-- sources: src/old.rs -->\n\nOld flow.\n";
    let ai = "# Auth\n\n## Tokens\n\n- opaque; see [[shared:decisions/0002#why]]\n";
    put(
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

    let paths = vec!["src/auth/token.rs".to_string(), "src/old.rs".to_string()];
    let c = app.section_context(&ctx, "app", &paths, Variant::Ai).await.unwrap();
    let got: Vec<_> = c
        .iter()
        .map(|s| (s.anchor.as_str(), s.variant, s.paths.clone(), s.links.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "tokens",
                Variant::Ai,
                vec!["src/auth/token.rs".to_string()],
                vec!["shared:decisions/0002#why".to_string()]
            ),
            // Only in the human variant.
            ("legacy", Variant::Human, vec!["src/old.rs".to_string()], vec![]),
        ]
    );
    assert!(c[0].content.starts_with("## Tokens"));

    let h = app
        .section_context(&ctx, "app", &paths[..1], Variant::Human)
        .await
        .unwrap();
    assert_eq!((h.len(), h[0].variant), (1, Variant::Human));
    assert_eq!(h[0].links, ["app:decisions/0001"]);
    assert!(
        app.section_context(&ctx, "app", &["/abs".to_string()], Variant::Ai)
            .await
            .is_err()
    );
}

/// `changes` until `done` holds. The window's upper bound is cluster-wide, so
/// transactions of concurrent tests can delay visibility briefly.
async fn changes_until(app: &App, ctx: &Ctx, project: &str, since: &str, done: impl Fn(&Changes) -> bool) -> Changes {
    for _ in 0..200 {
        let c = app.changes(ctx, project, Some(since)).await.unwrap();
        if done(&c) {
            return c;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("changes did not appear");
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn change_feed(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let start = app.changes(&ctx, "app", None).await.unwrap();
    assert!(start.documents.is_empty());
    let t0 = time::OffsetDateTime::now_utc() - time::Duration::seconds(5);

    let v1 = "# Guide\n\n## Keep\n\nSame.\n\n## Edit\n\nOld.\n\n## Drop\n\nGone soon.\n";
    put(&app, &ctx, "app", "guide", Variant::Human, v1, Expect::Absent, &[])
        .await
        .unwrap();
    put(
        &app,
        &ctx,
        "shared",
        "rules",
        Variant::Human,
        "# Rules\n\nBe brief.\n",
        Expect::Absent,
        &[],
    )
    .await
    .unwrap();
    let c = changes_until(&app, &ctx, "app", &start.cursor, |c| c.documents.len() == 2).await;
    let paths: Vec<_> = c
        .documents
        .iter()
        .map(|d| (d.path.as_str(), d.owner.as_str(), d.inherited, d.created))
        .collect();
    assert_eq!(paths, [("guide", "app", false, true), ("rules", "shared", true, true)]);
    assert_eq!(c.documents[0].variants[0].added, ["guide", "keep", "edit", "drop"]);
    assert_eq!(c.documents[0].variants[0].authors[0].kind, "system");
    // The parent project does not see the child's documents.
    let s = changes_until(&app, &ctx, "shared", &start.cursor, |c| !c.documents.is_empty()).await;
    assert_eq!(s.documents.len(), 1);

    // Net section changes across two revisions.
    let mid = c.cursor.clone();
    let v2 = "# Guide\n\n## Keep\n\nSame.\n\n## Edit\n\nNew.\n\n## Drop\n\nGone soon.\n";
    let v3 = "# Guide\n\n## Keep\n\nSame.\n\n## Edit\n\nNewer.\n\n## Added\n\nHello.\n";
    put(&app, &ctx, "app", "guide", Variant::Human, v2, Expect::Any, &[])
        .await
        .unwrap();
    put(&app, &ctx, "app", "guide", Variant::Human, v3, Expect::Any, &[])
        .await
        .unwrap();
    let c = changes_until(&app, &ctx, "app", &mid, |c| {
        c.documents.first().is_some_and(|d| d.variants[0].revisions == 2)
    })
    .await;
    assert_eq!(c.documents.len(), 1);
    let d = &c.documents[0];
    assert!(!d.created && !d.deleted);
    let v = &d.variants[0];
    assert_eq!((v.variant, v.content_hash), (Variant::Human, Hash::of(v3)));
    assert_eq!(
        (v.added.as_slice(), v.changed.as_slice(), v.removed.as_slice()),
        (
            &["added".to_string()][..],
            &["edit".to_string()][..],
            &["drop".to_string()][..]
        )
    );

    // An override shadows the inherited document; deleting reports the path.
    let mid = c.cursor.clone();
    put(
        &app,
        &ctx,
        "shared",
        "rules",
        Variant::Human,
        "# Rules\n\nShorter.\n",
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    put(
        &app,
        &ctx,
        "app",
        "rules",
        Variant::Human,
        "# Rules\n\nApp.\n",
        Expect::Any,
        &[],
    )
    .await
    .unwrap();
    app.delete_doc(&ctx, "app", &path("guide")).await.unwrap();
    let c = changes_until(&app, &ctx, "app", &mid, |c| c.documents.len() == 2).await;
    let got: Vec<_> = c
        .documents
        .iter()
        .map(|d| {
            (
                d.path.as_str(),
                d.owner.as_str(),
                d.created,
                d.deleted,
                d.variants.len(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [("guide", "app", false, true, 0), ("rules", "app", true, false, 1)]
    );

    // Nothing new after the last cursor; a timestamp covers everything since then.
    let last = app.changes(&ctx, "app", Some(&c.cursor)).await.unwrap();
    assert!(last.documents.is_empty());
    let all = app
        .changes(
            &ctx,
            "app",
            Some(&t0.format(&time::format_description::well_known::Rfc3339).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(all.documents.len(), 2);
    assert!(all.documents.iter().all(|d| d.created));
    assert!(matches!(
        app.changes(&ctx, "app", Some("yesterday")).await,
        Err(AppError::Invalid(_))
    ));
}

async fn restore(app: &App, ctx: &Ctx, p: &str, rev: RevisionId, companion: bool, dry_run: bool) -> RestoreResult {
    app.restore(
        ctx,
        Restore {
            project: "app",
            path: &path(p),
            revision: rev,
            expect: Expect::Any,
            message: None,
            companion,
            dry_run,
        },
    )
    .await
    .unwrap()
}

async fn head(app: &App, ctx: &Ctx, p: &str, v: Variant) -> String {
    app.get_doc(ctx, "app", &path(p), v)
        .await
        .unwrap()
        .head
        .unwrap()
        .content
}

fn states(sync: &[solidate_app::core::sync::SectionSync]) -> Vec<(&str, SyncState)> {
    sync.iter().map(|s| (s.anchor.as_str(), s.state)).collect()
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn restores(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, ctx) = setup(pool, opts).await;
    let h1 = "# Auth\n\nTokens are opaque.\n\n## Expiry\n\nSessions last 14 days.\n";
    let a1 = "# Auth\n\n- tokens: opaque\n\n## Expiry\n\n- session_ttl: 14d\n";
    let r1 = put(&app, &ctx, "app", "auth", Variant::Human, h1, Expect::Absent, &[])
        .await
        .unwrap()
        .revision;
    let ra1 = put(&app, &ctx, "app", "auth", Variant::Ai, a1, Expect::Absent, &[])
        .await
        .unwrap()
        .revision;

    // Undo before the AI caught up: back to the base, nothing else to do.
    let h2 = h1.replace("14 days", "30 days");
    put(&app, &ctx, "app", "auth", Variant::Human, &h2, Expect::Any, &[])
        .await
        .unwrap();
    let r = restore(&app, &ctx, "auth", r1.id, true, false).await;
    assert!(r.put.created);
    assert_eq!(r.put.revision.restored_from, Some(r1.id));
    assert!(r.companion.is_none());
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));
    assert_eq!(head(&app, &ctx, "auth", Variant::Human).await, h1);
    assert!(r.diff.contains("-Sessions last 30 days."), "{}", r.diff);

    // Undo after the AI was translated: the AI section goes back to the text that
    // was in sync with the restored one.
    let h3 = h1.replace("14 days", "7 days");
    put(&app, &ctx, "app", "auth", Variant::Human, &h3, Expect::Any, &[])
        .await
        .unwrap();
    let a3 = a1.replace("14d", "7d");
    put(&app, &ctx, "app", "auth", Variant::Ai, &a3, Expect::Any, &["expiry"])
        .await
        .unwrap();

    // Dry run: full result, nothing written.
    let d = restore(&app, &ctx, "auth", r1.id, true, true).await;
    assert!(d.dry_run);
    assert_eq!(
        d.companion.as_ref().map(|c| c.anchors.clone()),
        Some(vec!["expiry".to_owned()])
    );
    assert_eq!(head(&app, &ctx, "auth", Variant::Human).await, h3);
    assert_eq!(head(&app, &ctx, "auth", Variant::Ai).await, a3);

    // Without companion: the AI side is stale, and the sync item offers the paired
    // text.
    let r = restore(&app, &ctx, "auth", r1.id, false, false).await;
    assert_eq!(
        states(&r.sync),
        [("auth", SyncState::InSync), ("expiry", SyncState::HumanAhead)]
    );
    let item = app.sync_item(&ctx, "app", &path("auth"), "expiry").await.unwrap();
    assert_eq!(item.paired.as_deref(), Some("## Expiry\n\n- session_ttl: 14d\n"));

    // Back to h3 (paired with a3), then restore r1 with companion.
    let r3 = app
        .history(&ctx, "app", &path("auth"), Variant::Human, 10)
        .await
        .unwrap()
        .1
        .into_iter()
        .find(|r| r.content_hash == Hash::of(&h3))
        .unwrap();
    let r = restore(&app, &ctx, "auth", r3.id, true, false).await;
    assert!(r.reconciled.is_empty());
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));
    let r = restore(&app, &ctx, "auth", r1.id, true, false).await;
    let c = r.companion.unwrap();
    assert_eq!(c.anchors, ["expiry"]);
    assert_eq!(r.reconciled, ["expiry"]);
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));
    assert_eq!(head(&app, &ctx, "auth", Variant::Ai).await, a1);

    // AI side edited since the last sync: left for a person.
    let h4 = h1.replace("Tokens are opaque.", "Tokens are signed.");
    put(&app, &ctx, "app", "auth", Variant::Human, &h4, Expect::Any, &[])
        .await
        .unwrap();
    let a4 = a1.replace("opaque", "signed");
    put(&app, &ctx, "app", "auth", Variant::Ai, &a4, Expect::Any, &["auth"])
        .await
        .unwrap();
    let a5 = a4.replace("signed", "jwt");
    put(&app, &ctx, "app", "auth", Variant::Ai, &a5, Expect::Any, &[])
        .await
        .unwrap();
    let r = restore(&app, &ctx, "auth", r1.id, true, false).await;
    assert_eq!(r.skipped, ["auth"]);
    assert!(r.companion.is_none());
    assert_eq!(
        states(&r.sync),
        [("auth", SyncState::Conflict), ("expiry", SyncState::InSync)]
    );

    // Reverting a bad AI write that was marked in sync brings the earlier base back
    // instead of asking a person to update the human variant.
    put(&app, &ctx, "app", "auth", Variant::Ai, a1, Expect::Any, &["auth"])
        .await
        .unwrap();
    let bad = a1.replace("opaque", "wrong");
    put(&app, &ctx, "app", "auth", Variant::Ai, &bad, Expect::Any, &["auth"])
        .await
        .unwrap();
    let r = restore(&app, &ctx, "auth", ra1.id, true, false).await;
    assert_eq!(r.reconciled, ["auth"]);
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));

    // A section the restore removes is removed from the other variant.
    let h6 = format!("{h1}\n## Scopes\n\nRead and write.\n");
    let a6 = format!("{a1}\n## Scopes\n\n- scopes: read, write\n");
    put(&app, &ctx, "app", "auth", Variant::Human, &h6, Expect::Any, &[])
        .await
        .unwrap();
    put(&app, &ctx, "app", "auth", Variant::Ai, &a6, Expect::Any, &["scopes"])
        .await
        .unwrap();
    let r = restore(&app, &ctx, "auth", r1.id, true, false).await;
    assert_eq!(r.companion.unwrap().anchors, ["scopes"]);
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));
    // The blank line before the removed heading stays with the preceding section.
    assert_eq!(head(&app, &ctx, "auth", Variant::Ai).await.trim_end(), a1.trim_end());

    // Restoring it again inserts the paired section after its predecessor.
    let r6 = app
        .history(&ctx, "app", &path("auth"), Variant::Human, 50)
        .await
        .unwrap()
        .1
        .into_iter()
        .find(|r| r.content_hash == Hash::of(&h6))
        .unwrap();
    let r = restore(&app, &ctx, "auth", r6.id, true, false).await;
    assert_eq!(r.companion.unwrap().anchors, ["scopes"]);
    assert!(r.sync.iter().all(|s| s.state == SyncState::InSync));
    assert!(
        head(&app, &ctx, "auth", Variant::Ai)
            .await
            .ends_with("## Scopes\n\n- scopes: read, write\n")
    );

    // Restoring the current content writes nothing.
    let r = restore(&app, &ctx, "auth", r6.id, true, false).await;
    assert!(!r.put.created);
}
