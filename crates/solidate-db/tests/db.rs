//! Integration tests. Require `DATABASE_URL` (a superuser connection); see `.env.example`.

use solidate_core::{DocPath, Hash, Role, Scope, Slug, SyncBase, Variant, analyze};
use solidate_db::*;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

async fn db(pool: PgPoolOptions, opts: PgConnectOptions) -> Db {
    Db::connect_with(pool, opts).await.unwrap()
}

fn slug(s: &str) -> Slug {
    Slug::parse(s).unwrap()
}

fn path(s: &str) -> DocPath {
    DocPath::parse(s).unwrap()
}

async fn write(
    tx: &mut TenantTx,
    doc: DocumentId,
    variant: Variant,
    content: &str,
    expect: Expect,
) -> Result<(Revision, bool)> {
    let analysis = analyze(content, None);
    tx.write_revision(NewRevision {
        document: doc,
        variant,
        content,
        analysis: &analysis,
        author: Author::System,
        message: None,
        expect,
    })
    .await
}

#[sqlx::test(migrator = "solidate_db::MIGRATOR")]
async fn rls_isolates_tenants(pool: PgPoolOptions, opts: PgConnectOptions) {
    let db = db(pool, opts).await;
    let a = db.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let b = db.create_tenant(&slug("globex"), "Globex").await.unwrap();

    let mut tx = db.tenant(a.id).await.unwrap();
    let pa = tx.create_project(&slug("docs"), "Docs", None).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.tenant(b.id).await.unwrap();
    tx.create_project(&slug("docs"), "Docs", None).await.unwrap();
    assert!(
        tx.project(pa.id).await.unwrap().is_none(),
        "other tenant's project is invisible"
    );
    assert_eq!(tx.projects().await.unwrap().len(), 1);
    // Writing a row for another tenant violates the policy's WITH CHECK.
    let err = sqlx::query("INSERT INTO projects (tenant_id, slug, name) VALUES ($1, 'x', 'x')")
        .bind(a.id)
        .execute(db.pool())
        .await;
    assert!(err.is_err());
    tx.commit().await.unwrap();

    // Without a tenant set, tenant-scoped tables and the tenants table are empty.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(db.tenant_by_slug("acme").await.unwrap().unwrap().id, a.id);
}

#[sqlx::test(migrator = "solidate_db::MIGRATOR")]
async fn revisions_heads_sections_links(pool: PgPoolOptions, opts: PgConnectOptions) {
    let db = db(pool, opts).await;
    let t = db.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let mut tx = db.tenant(t.id).await.unwrap();
    let p = tx.create_project(&slug("app"), "App", None).await.unwrap();
    let doc = tx.create_document(p.id, &path("design/auth")).await.unwrap();
    let other = tx.create_document(p.id, &path("design/sessions")).await.unwrap();

    let v1 = "# Auth\n\nTokens are opaque.\n\n## Sessions\n\nSee [[design/sessions]].\n";
    let (r1, created) = write(&mut tx, doc.id, Variant::Human, v1, Expect::Absent)
        .await
        .unwrap();
    assert!(created);

    let head = tx.head(doc.id, Variant::Human).await.unwrap().unwrap();
    assert_eq!((head.content.as_str(), head.content_hash), (v1, Hash::of(v1)));
    assert_eq!(
        tx.document(doc.id).await.unwrap().unwrap().title.as_deref(),
        Some("Auth")
    );
    let anchors: Vec<_> = tx
        .sections(r1.id)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.anchor)
        .collect();
    assert_eq!(anchors, ["auth", "sessions"]);

    // Same content: no new revision.
    let (same, created) = write(&mut tx, doc.id, Variant::Human, v1, Expect::Any).await.unwrap();
    assert!(!created);
    assert_eq!(same.id, r1.id);

    // Stale precondition.
    let err = write(
        &mut tx,
        doc.id,
        Variant::Human,
        "# x\n",
        Expect::Head(Hash::of("stale")),
    )
    .await;
    assert!(matches!(err, Err(DbError::HeadMismatch { current: Some(h) }) if h == Hash::of(v1)));
    let err = write(&mut tx, doc.id, Variant::Human, "# x\n", Expect::Absent).await;
    assert!(matches!(err, Err(DbError::HeadMismatch { .. })));

    let v2 = "# Auth\n\nTokens are opaque and rotated.\n";
    let (r2, _) = write(&mut tx, doc.id, Variant::Human, v2, Expect::Head(Hash::of(v1)))
        .await
        .unwrap();
    assert_eq!(r2.parent_id, Some(r1.id));
    let history: Vec<_> = tx
        .revisions(doc.id, Variant::Human, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(history, [r2.id, r1.id]);
    assert_eq!(tx.blob(Hash::of(v1)).await.unwrap().as_deref(), Some(v1));

    // v2 dropped the link; add it back from the AI variant.
    assert!(tx.backlinks(&p, &path("design/sessions")).await.unwrap().is_empty());
    write(
        &mut tx,
        doc.id,
        Variant::Ai,
        "# Auth\n\n- see [[design/sessions]]\n",
        Expect::Absent,
    )
    .await
    .unwrap();
    let bl = tx.backlinks(&p, &path("design/sessions")).await.unwrap();
    assert_eq!(
        (bl.len(), bl[0].path.as_str(), bl[0].variant),
        (1, "design/auth", Variant::Ai)
    );

    let docs = tx.documents(p.id).await.unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].human_hash, Some(Hash::of(v2)));
    assert!(docs[1].human_hash.is_none());

    let hits = tx.search("rotated", None, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet_html().contains("<mark>rotated</mark>"));

    tx.delete_document(other.id).await.unwrap();
    assert_eq!(tx.documents(p.id).await.unwrap().len(), 1);
    // Path can be reused after deletion.
    tx.create_document(p.id, &path("design/sessions")).await.unwrap();
    assert!(matches!(
        tx.create_document(p.id, &path("design/sessions")).await,
        Err(DbError::AlreadyExists(_))
    ));
}

#[sqlx::test(migrator = "solidate_db::MIGRATOR")]
async fn project_chain_and_cycles(pool: PgPoolOptions, opts: PgConnectOptions) {
    let db = db(pool, opts).await;
    let t = db.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let mut tx = db.tenant(t.id).await.unwrap();
    let root = tx.create_project(&slug("shared"), "Shared", None).await.unwrap();
    let mid = tx
        .create_project(&slug("platform"), "Platform", Some(root.id))
        .await
        .unwrap();
    let leaf = tx.create_project(&slug("api"), "API", Some(mid.id)).await.unwrap();

    let chain: Vec<_> = tx
        .project_chain(leaf.id)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.slug)
        .collect();
    assert_eq!(chain, ["api", "platform", "shared"]);
    assert!(matches!(
        tx.set_project_parent(root.id, Some(leaf.id)).await,
        Err(DbError::Invalid(_))
    ));
    tx.set_project_parent(leaf.id, Some(root.id)).await.unwrap();
    assert_eq!(tx.project_chain(leaf.id).await.unwrap().len(), 2);
}

#[sqlx::test(migrator = "solidate_db::MIGRATOR")]
async fn sync_bases_round_trip(pool: PgPoolOptions, opts: PgConnectOptions) {
    let db = db(pool, opts).await;
    let t = db.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let mut tx = db.tenant(t.id).await.unwrap();
    let p = tx.create_project(&slug("app"), "App", None).await.unwrap();
    let d = tx.create_document(p.id, &path("readme")).await.unwrap();

    let b1 = SyncBase {
        human: Some(Hash::of("h")),
        ai: Some(Hash::of("a")),
    };
    let b2 = SyncBase {
        human: Some(Hash::of("h2")),
        ai: None,
    };
    tx.put_sync_bases(d.id, &[("x".into(), b1), ("y".into(), b2)])
        .await
        .unwrap();
    let got = tx.sync_bases(d.id).await.unwrap();
    assert_eq!((got["x"], got["y"]), (b1, b2));

    tx.put_sync_bases(d.id, &[("y".into(), SyncBase::default())])
        .await
        .unwrap();
    assert!(!tx.sync_bases(d.id).await.unwrap().contains_key("y"));
    tx.prune_sync_bases(d.id, &[]).await.unwrap();
    assert!(tx.sync_bases(d.id).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "solidate_db::MIGRATOR")]
async fn users_sessions_memberships_tokens(pool: PgPoolOptions, opts: PgConnectOptions) {
    let db = db(pool, opts).await;
    let t = db.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let u = db.create_user("Ada@Example.com", "Ada", "hash").await.unwrap();
    assert_eq!(db.user_by_email("ada@example.com").await.unwrap().unwrap().id, u.id);
    assert!(matches!(
        db.create_user("ADA@example.com", "x", "y").await,
        Err(DbError::AlreadyExists(_))
    ));

    let future = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    db.create_session(b"tok", u.id, future).await.unwrap();
    assert_eq!(db.session(b"tok").await.unwrap().unwrap().user.id, u.id);
    db.extend_session(b"tok", time::OffsetDateTime::now_utc() - time::Duration::seconds(1))
        .await
        .unwrap();
    assert!(db.session(b"tok").await.unwrap().is_none(), "expired");
    assert_eq!(db.delete_expired_sessions().await.unwrap(), 1);

    let mut tx = db.tenant(t.id).await.unwrap();
    tx.set_membership(u.id, Role::Editor).await.unwrap();
    assert_eq!(tx.role(u.id).await.unwrap(), Some(Role::Editor));
    let tok = tx
        .create_token(NewToken {
            name: "ci",
            prefix: "sol_abc",
            secret_hash: b"secret",
            scopes: &[Scope::Read, Scope::Write],
            project: None,
            created_by: Some(u.id),
            expires_at: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let tenants = db.user_tenants(u.id).await.unwrap();
    assert_eq!(
        (tenants[0].tenant.slug.as_str(), tenants[0].role),
        ("acme", Role::Editor)
    );
    let creds = db.token_by_prefix("sol_abc").await.unwrap().unwrap();
    assert_eq!(
        (creds.id, creds.tenant_id, creds.scopes.as_slice()),
        (tok.id, t.id, &[Scope::Read, Scope::Write][..])
    );

    let mut tx = db.tenant(t.id).await.unwrap();
    tx.revoke_token(tok.id).await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        db.token_by_prefix("sol_abc")
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some()
    );
}

#[sqlx::test(migrator = "solidate_db::MIGRATOR")]
async fn audit_log_is_append_only_and_isolated(pool: PgPoolOptions, opts: PgConnectOptions) {
    let db = db(pool, opts).await;
    let a = db.create_tenant(&slug("acme"), "Acme").await.unwrap();
    let b = db.create_tenant(&slug("globex"), "Globex").await.unwrap();

    let mut tx = db.tenant(a.id).await.unwrap();
    let p = tx.create_project(&slug("docs"), "Docs", None).await.unwrap();
    for action in ["one", "two", "three"] {
        tx.audit(NewAudit {
            actor: Author::System,
            action,
            project: Some(p.id),
            target: Some("x"),
            detail: serde_json::json!({ "n": action }),
        })
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let mut tx = db.tenant(a.id).await.unwrap();
    let all = tx.audit_entries(10, None).await.unwrap();
    assert_eq!(
        all.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(),
        ["three", "two", "one"]
    );
    assert_eq!(all[0].project_slug.as_deref(), Some("docs"));
    let older = tx.audit_entries(10, Some(all[1].id)).await.unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].action, "one");
    tx.commit().await.unwrap();

    // The app role has no UPDATE or DELETE privilege on the log.
    for stmt in ["UPDATE audit_log SET action = 'x'", "DELETE FROM audit_log"] {
        let err = sqlx::query(stmt).execute(db.pool()).await.unwrap_err();
        assert!(err.to_string().contains("permission denied"), "{stmt}: {err}");
    }

    let mut tx = db.tenant(b.id).await.unwrap();
    assert!(tx.audit_entries(10, None).await.unwrap().is_empty());
}
