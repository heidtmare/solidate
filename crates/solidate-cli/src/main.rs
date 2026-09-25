//! `solidate` administration CLI. Operates with system privileges; connects with
//! `DATABASE_URL` (the login role must be allowed to `SET ROLE solidate_app`).

mod files;

use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use solidate_app::core::{DocPath, Role, Scope, Slug, Variant};
use solidate_app::db::{Db, Expect};
use solidate_app::{App, Config, Ctx, PutDoc, telemetry};
use time::{Duration, OffsetDateTime};

#[derive(Parser)]
#[command(name = "solidate", version, about = "Solidate administration")]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Apply pending database migrations.
    Migrate,
    #[command(subcommand)]
    Tenant(TenantCmd),
    #[command(subcommand)]
    User(UserCmd),
    #[command(subcommand)]
    Project(ProjectCmd),
    #[command(subcommand)]
    Token(TokenCmd),
    /// Import a directory of Markdown files (`x.md` human, `x.ai.md` AI variant).
    Import {
        tenant: String,
        project: String,
        dir: PathBuf,
        /// Print what would be imported without writing.
        #[arg(long)]
        dry_run: bool,
        /// Mark sections present in both variants of an imported document as in sync.
        #[arg(long)]
        synced: bool,
    },
    /// Export a project's own documents to a directory.
    Export {
        tenant: String,
        project: String,
        dir: PathBuf,
    },
    /// List sections needing sync in a project.
    SyncQueue { tenant: String, project: String },
    /// Print a tenant's audit log, newest first (tab-separated).
    Audit {
        tenant: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Continue after this entry id.
        #[arg(long)]
        before: Option<String>,
    },
}

#[derive(Subcommand)]
enum TenantCmd {
    Create { slug: String, name: String },
}

#[derive(Subcommand)]
enum UserCmd {
    /// Create a user. The password is read from `SOLIDATE_PASSWORD` or stdin.
    Create { email: String, name: String },
    /// Set a user's password (from `SOLIDATE_PASSWORD` or stdin).
    Passwd { email: String },
    /// Add a user to a tenant or change their role.
    Member {
        tenant: String,
        email: String,
        role: String,
    },
}

#[derive(Subcommand)]
enum ProjectCmd {
    Create {
        tenant: String,
        slug: String,
        name: String,
        #[arg(long)]
        parent: Option<String>,
    },
    List {
        tenant: String,
    },
    SetParent {
        tenant: String,
        slug: String,
        /// Omit to make the project a root.
        parent: Option<String>,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Create an API token and print its secret once.
    Create {
        tenant: String,
        name: String,
        #[arg(long = "scope", required = true, value_parser = parse_scope)]
        scopes: Vec<Scope>,
        /// Restrict the token to one project.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        expires_days: Option<i64>,
    },
    List {
        tenant: String,
    },
    Revoke {
        tenant: String,
        id: String,
    },
}

fn parse_scope(s: &str) -> Result<Scope, String> {
    s.parse().map_err(|e| format!("{e}"))
}

fn read_password() -> Result<String> {
    if let Ok(p) = std::env::var("SOLIDATE_PASSWORD") {
        return Ok(p);
    }
    eprint!("password: ");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    telemetry::init("warn");
    let cli = Cli::parse();
    if let Cmd::Migrate = cli.cmd {
        Db::migrate(&cli.database_url).await?;
        println!("migrations applied");
        return Ok(());
    }
    let app = App::new(Db::connect(&cli.database_url).await?, Config::default());
    run(&app, cli.cmd).await
}

async fn run(app: &App, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Migrate => unreachable!("handled in main"),
        Cmd::Tenant(TenantCmd::Create { slug, name }) => {
            let t = app.create_tenant(&Slug::parse(&slug)?, &name).await?;
            println!("{}\t{}", t.id, t.slug);
        }
        Cmd::User(UserCmd::Create { email, name }) => {
            let u = app.register_user(&email, &name, &read_password()?).await?;
            println!("{}\t{}", u.id, u.email);
        }
        Cmd::User(UserCmd::Passwd { email }) => {
            let u = app
                .db()
                .user_by_email(&email)
                .await?
                .ok_or_else(|| anyhow!("no user {email}"))?;
            app.set_password(u.id, &read_password()?).await?;
            println!("password updated");
        }
        Cmd::User(UserCmd::Member { tenant, email, role }) => {
            let role: Role = role.parse()?;
            let u = app
                .db()
                .user_by_email(&email)
                .await?
                .ok_or_else(|| anyhow!("no user {email}"))?;
            app.set_member(&app.system_ctx(&tenant).await?, u.id, role).await?;
            println!("{email} is {role} in {tenant}");
        }
        Cmd::Project(ProjectCmd::Create {
            tenant,
            slug,
            name,
            parent,
        }) => {
            let ctx = app.system_ctx(&tenant).await?;
            let p = app
                .create_project(&ctx, &Slug::parse(&slug)?, &name, parent.as_deref())
                .await?;
            println!("{}\t{}", p.id, p.slug);
        }
        Cmd::Project(ProjectCmd::List { tenant }) => {
            let ctx = app.system_ctx(&tenant).await?;
            let all = app.projects(&ctx).await?;
            for p in &all {
                let parent = p
                    .parent_id
                    .and_then(|id| all.iter().find(|x| x.id == id))
                    .map_or("-", |x| &x.slug);
                println!("{}\t{}\tparent={parent}", p.slug, p.name);
            }
        }
        Cmd::Project(ProjectCmd::SetParent { tenant, slug, parent }) => {
            let ctx = app.system_ctx(&tenant).await?;
            app.set_project_parent(&ctx, &slug, parent.as_deref()).await?;
            println!("updated");
        }
        Cmd::Token(TokenCmd::Create {
            tenant,
            name,
            scopes,
            project,
            expires_days,
        }) => {
            let ctx = app.system_ctx(&tenant).await?;
            let project = match project {
                Some(slug) => Some(app.project(&ctx, &slug).await?.id),
                None => None,
            };
            let expires = expires_days.map(|d| OffsetDateTime::now_utc() + Duration::days(d));
            let t = app.create_api_token(&ctx, &name, &scopes, project, expires).await?;
            eprintln!("token {} created; the secret is shown once:", t.token.id);
            println!("{}", t.secret);
        }
        Cmd::Token(TokenCmd::List { tenant }) => {
            let ctx = app.system_ctx(&tenant).await?;
            for t in app.api_tokens(&ctx).await? {
                let scopes: Vec<_> = t.scopes.iter().map(|s| s.as_str()).collect();
                let status = if t.revoked_at.is_some() { "revoked" } else { "active" };
                println!("{}\t{}\t{}\t{}\t{status}", t.id, t.prefix, t.name, scopes.join(","));
            }
        }
        Cmd::Token(TokenCmd::Revoke { tenant, id }) => {
            let ctx = app.system_ctx(&tenant).await?;
            app.revoke_api_token(&ctx, id.parse().context("invalid token id")?)
                .await?;
            println!("revoked");
        }
        Cmd::Import {
            tenant,
            project,
            dir,
            dry_run,
            synced,
        } => {
            let ctx = app.system_ctx(&tenant).await?;
            import(app, &ctx, &project, &dir, dry_run, synced).await?;
        }
        Cmd::Export { tenant, project, dir } => {
            let ctx = app.system_ctx(&tenant).await?;
            export(app, &ctx, &project, &dir).await?;
        }
        Cmd::SyncQueue { tenant, project } => {
            let ctx = app.system_ctx(&tenant).await?;
            for e in app.sync_queue(&ctx, &project).await? {
                let side = e.stale_side.map_or("both", |v| v.as_str());
                println!("{}#{}\t{}\tstale={side}", e.path, e.anchor, e.state);
            }
        }
        Cmd::Audit { tenant, limit, before } => {
            let ctx = app.system_ctx(&tenant).await?;
            let before = before
                .map(|b| b.parse())
                .transpose()
                .context("invalid audit entry id")?;
            for e in app.audit_log(&ctx, limit, before).await? {
                let actor = match (e.actor_user_id, e.actor_token_id) {
                    (Some(u), _) => format!("user:{u}"),
                    (_, Some(t)) => format!("token:{t}"),
                    _ => e.actor_kind.clone(),
                };
                let at = e.at.format(&time::format_description::well_known::Rfc3339)?;
                println!(
                    "{}\t{at}\t{actor}\t{}\t{}\t{}\t{}",
                    e.id,
                    e.action,
                    e.project_slug.as_deref().unwrap_or("-"),
                    e.target.as_deref().unwrap_or("-"),
                    e.detail
                );
            }
        }
    }
    Ok(())
}

async fn import(app: &App, ctx: &Ctx, project: &str, dir: &Path, dry_run: bool, synced: bool) -> Result<()> {
    app.project(ctx, project).await?;
    let mut files: Vec<(DocPath, Variant, PathBuf)> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(dir)?;
        match files::doc_for_file(rel) {
            Some((path, variant)) => files.push((path, variant, entry.path().to_owned())),
            None if rel.extension().is_some_and(|e| e == "md") => eprintln!("skip {}: invalid path", rel.display()),
            None => {}
        }
    }
    // Human variants first so sync state starts from the human side.
    files.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    // Documents with both variants in this import.
    let paired: Vec<DocPath> = files
        .iter()
        .filter(|(p, v, _)| *v == Variant::Ai && files.iter().any(|(q, w, _)| q == p && *w == Variant::Human))
        .map(|(p, _, _)| p.clone())
        .collect();

    let (mut written, mut unchanged) = (0, 0);
    for (path, variant, file) in files {
        let content = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        if dry_run {
            println!("would import {path} ({variant})");
            continue;
        }
        let r = app
            .put_doc(
                ctx,
                PutDoc {
                    project,
                    path: &path,
                    variant,
                    content: &content,
                    expect: Expect::Any,
                    message: Some("import"),
                    resolves: &[],
                },
            )
            .await
            .with_context(|| format!("importing {path} ({variant})"))?;
        if r.created {
            written += 1;
            println!("imported {path} ({variant})");
        } else {
            unchanged += 1;
        }
    }
    if !dry_run {
        println!("{written} written, {unchanged} unchanged");
    }
    if synced && !dry_run {
        let mut reconciled = 0;
        for path in paired {
            reconciled += app
                .resolve_paired_sync(ctx, project, &path)
                .await
                .with_context(|| format!("marking {path} in sync"))?;
        }
        println!("{reconciled} sections marked in sync");
    }
    Ok(())
}

async fn export(app: &App, ctx: &Ctx, project: &str, dir: &Path) -> Result<()> {
    let tree = app.tree(ctx, project).await?;
    let mut n = 0;
    for entry in tree.entries.iter().filter(|e| !e.inherited) {
        let path = DocPath::parse(&entry.path)?;
        for variant in Variant::ALL {
            let Some(head) = app.get_doc(ctx, project, &path, variant).await?.head else {
                continue;
            };
            let file = files::file_for_doc(dir, &path, variant);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file, head.content)?;
            n += 1;
        }
    }
    println!("{n} files written to {}", dir.display());
    Ok(())
}
