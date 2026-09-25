//! MCP server over [`solidate_app`]. Tools map one-to-one to app operations; all
//! authorization happens in the app layer through the caller's API token.
//!
//! Transports:
//! - stdio ([`SolidateMcp::with_token`]): one fixed token for the process.
//! - streamable HTTP ([`http_service`]): the host server authenticates each request.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Extensions, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use solidate_app::core::{DocPath, Variant, analyze};
use solidate_app::db::Expect;
use solidate_app::{App, AppError, Ctx, PutDoc};
use time::format_description::well_known::Rfc3339;

const INSTRUCTIONS: &str = "Solidate stores project design documents. Each document has a human variant (narrative) \
and an AI variant (dense, structured), paired by section anchors. Read with read_doc; write with write_doc, passing \
the content_hash from your last read as base_hash (optimistic concurrency). When one variant changes, its sections \
appear in get_sync_queue; propagate the change to the other variant with write_doc and list the section anchors in \
`resolves`. Projects inherit documents from parent projects; {{include project:path#anchor}} transcludes content.";

type ToolResult = Result<CallToolResult, McpError>;

#[derive(Clone)]
pub struct SolidateMcp {
    app: App,
    /// Fixed token (stdio). `None`: the HTTP layer authenticates each request and
    /// stores the [`Ctx`] in the request extensions.
    token: Option<Arc<str>>,
    tool_router: ToolRouter<Self>,
}

fn ok(v: Value) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::json(v)?]))
}

fn text(s: String) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}

/// App errors are reported as tool errors so the model can react to them.
fn fail(e: AppError) -> ToolResult {
    let msg = match e {
        AppError::PreconditionFailed { current: Some(h) } => format!(
            "precondition failed: the document changed; current content_hash is {}. Re-read and retry.",
            h.to_hex()
        ),
        AppError::PreconditionFailed { current: None } => {
            "precondition failed: the document variant already exists or was removed; re-read and retry.".to_owned()
        }
        AppError::Internal(m) => {
            tracing::error!(error = %m, "mcp tool internal error");
            "internal error".to_owned()
        }
        e => e.to_string(),
    };
    Ok(CallToolResult::error(vec![ContentBlock::text(msg)]))
}

macro_rules! try_app {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return fail(e.into()),
        }
    };
}

fn parse_path(path: &str) -> Result<DocPath, AppError> {
    DocPath::parse(path).map_err(|e| AppError::Invalid(e.to_string()))
}

fn parse_variant(v: Option<&str>) -> Result<Variant, AppError> {
    match v {
        None | Some("human") => Ok(Variant::Human),
        Some("ai") => Ok(Variant::Ai),
        Some(o) => Err(AppError::Invalid(format!(
            "unknown variant {o:?}; expected human or ai"
        ))),
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ProjectArg {
    /// Project slug.
    pub project: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Full-text query (web search syntax: quotes, OR, -exclude).
    pub query: String,
    /// Limit to one project.
    pub project: Option<String>,
    /// Maximum results (default 20, max 100).
    pub limit: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReadDocArgs {
    pub project: String,
    /// Document path, e.g. `design/auth`.
    pub path: String,
    /// `human` (default) or `ai`.
    pub variant: Option<String>,
    /// Return only the section with this anchor.
    pub section: Option<String>,
    /// Expand `{{include}}` directives.
    #[serde(default)]
    pub expand: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct WriteDocArgs {
    pub project: String,
    pub path: String,
    /// `human` (default) or `ai`.
    pub variant: Option<String>,
    /// Full Markdown content of the variant.
    pub content: String,
    /// `content_hash` of the variant as last read. Omit only when creating the variant.
    pub base_hash: Option<String>,
    /// Change note.
    pub message: Option<String>,
    /// Section anchors this write propagates; they are marked in sync afterwards.
    #[serde(default)]
    pub resolves: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct DocArgs {
    pub project: String,
    pub path: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SyncItemArgs {
    pub project: String,
    pub path: String,
    /// Section anchor.
    pub anchor: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ResolveArgs {
    pub project: String,
    pub path: String,
    /// Anchors to mark in sync without editing.
    #[serde(default)]
    pub anchors: Vec<String>,
    /// Instead of `anchors`: every pending section present in both variants.
    #[serde(default)]
    pub paired: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct HistoryArgs {
    pub project: String,
    pub path: String,
    /// `human` (default) or `ai`.
    pub variant: Option<String>,
    /// Maximum revisions (default 20).
    pub limit: Option<i64>,
}

impl SolidateMcp {
    /// Server authenticating every call with `token` (stdio).
    pub fn with_token(app: App, token: impl Into<Arc<str>>) -> Self {
        Self {
            app,
            token: Some(token.into()),
            tool_router: Self::tool_router(),
        }
    }

    /// Server reading the [`Ctx`] placed in request extensions by [`http_service`].
    fn for_http(app: App) -> Self {
        Self {
            app,
            token: None,
            tool_router: Self::tool_router(),
        }
    }

    async fn ctx(&self, ext: &Extensions) -> Result<Ctx, AppError> {
        match &self.token {
            Some(t) => self.app.token_ctx(t).await,
            None => ext
                .get::<http::request::Parts>()
                .and_then(|p| p.extensions.get::<Ctx>())
                .cloned()
                .ok_or(AppError::Unauthorized),
        }
    }
}

#[tool_router]
impl SolidateMcp {
    #[tool(description = "List projects visible to this token, with their parent (inheritance) project.")]
    async fn list_projects(&self, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let all = try_app!(self.app.projects(&ctx).await);
        let out: Vec<Value> = all
            .iter()
            .map(|p| {
                let parent = p
                    .parent_id
                    .and_then(|id| all.iter().find(|x| x.id == id))
                    .map(|x| &x.slug);
                json!({ "slug": p.slug, "name": p.name, "parent": parent })
            })
            .collect();
        ok(json!(out))
    }

    #[tool(
        description = "List a project's effective documents (own and inherited) with per-variant content hashes and the project root hash."
    )]
    async fn list_docs(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let tree = try_app!(self.app.tree(&ctx, &a.project).await);
        ok(json!({ "project": tree.project.slug, "root_hash": tree.root_hash, "entries": tree.entries }))
    }

    #[tool(
        description = "Merkle root hash over a project's effective documents. Unchanged hash means nothing the project sees has changed."
    )]
    async fn project_hash(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let tree = try_app!(self.app.tree(&ctx, &a.project).await);
        ok(json!({ "root_hash": tree.root_hash }))
    }

    #[tool(description = "Full-text search across documents.")]
    async fn search(&self, Parameters(a): Parameters<SearchArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let hits = try_app!(
            self.app
                .search(&ctx, &a.query, a.project.as_deref(), a.limit.unwrap_or(20))
                .await
        );
        let out: Vec<Value> = hits
            .iter()
            .map(|h| {
                json!({
                    "project": h.project_slug, "path": h.path, "variant": h.variant,
                    "title": h.title, "snippet": h.snippet_text(),
                })
            })
            .collect();
        ok(json!(out))
    }

    #[tool(
        description = "Read one variant of a document, or one section of it. Returns content, content_hash (pass as base_hash when writing), and the section outline with semantic hashes."
    )]
    async fn read_doc(&self, Parameters(a): Parameters<ReadDocArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let variant = try_app!(parse_variant(a.variant.as_deref()));
        let e = try_app!(self.app.expanded_doc(&ctx, &a.project, &path, variant).await);
        let Some(head) = &e.view.head else {
            return fail(AppError::Invalid(format!(
                "the {variant} variant of {path} has not been written"
            )));
        };
        let content = if a.expand {
            e.text.as_deref().unwrap_or_default()
        } else {
            head.content.as_str()
        };
        let analysis = analyze(content, Some(&path));
        if let Some(anchor) = &a.section {
            let Some(s) = analysis.section(anchor) else {
                return fail(AppError::Invalid(format!("no section #{anchor} in {path}")));
            };
            return ok(json!({
                "path": path.as_str(), "variant": variant, "content_hash": head.content_hash,
                "anchor": s.anchor, "title": s.title, "hash": s.hash, "content": s.body,
            }));
        }
        let sections: Vec<Value> = analysis
            .sections
            .iter()
            .map(|s| json!({ "anchor": s.anchor, "title": s.title, "level": s.level, "hash": s.hash }))
            .collect();
        ok(json!({
            "project": e.view.project.slug,
            "owner": e.view.owner.slug,
            "inherited": e.view.inherited(),
            "path": path.as_str(),
            "variant": variant,
            "title": head.title,
            "content_hash": head.content_hash,
            "resolved_hash": if a.expand { e.resolved_hash } else { None },
            "sections": sections,
            "content": content,
        }))
    }

    #[tool(
        description = "Write the full content of one document variant. Requires base_hash (the content_hash last read) unless creating the variant. Writing an inherited path creates an override in this project. List propagated section anchors in `resolves` to mark them in sync."
    )]
    async fn write_doc(&self, Parameters(a): Parameters<WriteDocArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let variant = try_app!(parse_variant(a.variant.as_deref()));
        let expect = match a.base_hash.as_deref() {
            Some(h) => Expect::Head(try_app!(
                h.parse()
                    .map_err(|_| AppError::Invalid("base_hash must be a 64-character hex hash".into()))
            )),
            None => Expect::Absent,
        };
        let r = try_app!(
            self.app
                .put_doc(
                    &ctx,
                    PutDoc {
                        project: &a.project,
                        path: &path,
                        variant,
                        content: &a.content,
                        expect,
                        message: a.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                        resolves: &a.resolves,
                    },
                )
                .await
        );
        let pending: Vec<Value> = r
            .sync
            .iter()
            .filter(|s| s.state.needs_attention())
            .map(|s| json!({ "anchor": s.anchor, "state": s.state, "stale_side": s.state.stale_side() }))
            .collect();
        ok(json!({
            "path": r.document.path,
            "variant": variant,
            "content_hash": r.revision.content_hash,
            "changed": r.created,
            "sync_pending": pending,
        }))
    }

    #[tool(
        description = "Sections whose human and AI variants diverged since the last sync, across a project's own documents. stale_side is the variant that needs updating; null means conflict."
    )]
    async fn get_sync_queue(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        ok(json!(try_app!(self.app.sync_queue(&ctx, &a.project).await)))
    }

    #[tool(
        description = "Everything needed to propagate one section: both variants' current text, text at the last sync, diffs since then, and each variant's content_hash for base_hash."
    )]
    async fn get_sync_item(&self, Parameters(a): Parameters<SyncItemArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        ok(json!(try_app!(
            self.app.sync_item(&ctx, &a.project, &path, &a.anchor).await
        )))
    }

    #[tool(
        description = "Mark sections as in sync without editing, when the variants already agree. Pass anchors, or paired=true for every pending section present in both variants."
    )]
    async fn resolve_sync(&self, Parameters(a): Parameters<ResolveArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        match (a.anchors.is_empty(), a.paired) {
            (false, false) => {
                try_app!(self.app.resolve_sync(&ctx, &a.project, &path, &a.anchors).await);
            }
            (true, true) => {
                try_app!(self.app.resolve_paired_sync(&ctx, &a.project, &path).await);
            }
            _ => return fail(AppError::Invalid("pass either anchors or paired=true".into())),
        }
        ok(json!(try_app!(self.app.doc_sync(&ctx, &a.project, &path).await)))
    }

    #[tool(description = "Documents linking to a document.")]
    async fn backlinks(&self, Parameters(a): Parameters<DocArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let links = try_app!(self.app.backlinks(&ctx, &a.project, &path).await);
        let out: Vec<Value> = links
            .iter()
            .map(|l| json!({ "project": l.project_slug, "path": l.path, "variant": l.variant, "anchor": l.target_anchor }))
            .collect();
        ok(json!(out))
    }

    #[tool(description = "Revisions of one document variant, newest first.")]
    async fn doc_history(&self, Parameters(a): Parameters<HistoryArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let variant = try_app!(parse_variant(a.variant.as_deref()));
        let (_, revs) = try_app!(
            self.app
                .history(&ctx, &a.project, &path, variant, a.limit.unwrap_or(20))
                .await
        );
        let out: Vec<Value> = revs
            .iter()
            .map(|r| {
                json!({
                    "revision": r.id.to_string(), "content_hash": r.content_hash,
                    "message": r.message, "created_at": r.created_at.format(&Rfc3339).ok(),
                })
            })
            .collect();
        ok(json!(out))
    }

    #[tool(description = "The llms.txt index of a project: one line per document with its title and path.")]
    async fn project_index(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let tree = try_app!(self.app.tree(&ctx, &a.project).await);
        let mut out = format!("# {}\n\n", tree.project.name);
        for e in &tree.entries {
            let variants = match (e.human.is_some(), e.ai.is_some()) {
                (true, true) => "human, ai",
                (false, true) => "ai",
                _ => "human",
            };
            let inherited = if e.inherited {
                format!("; inherited from {}", e.owner)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "- {}: `{}` ({variants}{inherited})\n",
                e.title.as_deref().unwrap_or(&e.path),
                e.path
            ));
        }
        text(out)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SolidateMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("solidate", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Streamable HTTP MCP service (stateless, JSON responses). The caller must
/// authenticate each request and insert its [`Ctx`] into the request extensions;
/// requests without one fail with an authentication error. `allowed_hosts` lists
/// accepted `Host` values (DNS rebinding protection).
pub fn http_service(app: App, allowed_hosts: Vec<String>) -> StreamableHttpService<SolidateMcp, NeverSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts);
    StreamableHttpService::new(
        move || Ok(SolidateMcp::for_http(app.clone())),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}
