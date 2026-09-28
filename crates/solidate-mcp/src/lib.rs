//! MCP server over [`solidate_app`]. Tools map one-to-one to app operations; all
//! authorization happens in the app layer through the caller's credential.
//!
//! Transports:
//! - stdio ([`SolidateMcp::with_bearer`]): one fixed bearer credential for the process.
//! - streamable HTTP ([`http_service`]): the host server authenticates each request.

use std::sync::Arc;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Extensions, GetPromptResult, Implementation, PromptMessage, Role, ServerCapabilities,
    ServerConfig,
};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{
    ErrorData as McpError, ServerHandler, prompt, prompt_handler, prompt_router, schemars, tool, tool_handler,
    tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use solidate_app::core::edit::span_hash;
use solidate_app::core::sources::Files;
use solidate_app::core::{DocPath, Hash, SectionTarget, Variant, analyze};
use solidate_app::db::Expect;
use solidate_app::{App, AppError, Credential, Ctx, Propose, PutDoc, PutSection, SourceReport};
use time::format_description::well_known::Rfc3339;

const INSTRUCTIONS: &str = "Solidate stores project documentation as one ground truth written twice: every document \
has a human variant (narrative) and an AI variant (dense, structured). The variants are translations of each other, \
paired by section anchors; they must state the same facts. Find sections with search (each hit carries the \
section's anchor and section_hash; pass variant \"ai\" to search the reference variant only). Read with read_doc, \
which reads the AI variant unless you ask for another; write with write_doc, passing the content_hash from your \
last read as base_hash (optimistic concurrency).\n\n\
To change part of a document, prefer write_section: read the section with read_doc(section=anchor), then send only \
its new Markdown with the section's `hash` as section_hash. Edits to other sections in the meantime do not conflict. \
Keep the heading text (or an explicit `{#anchor}`) so the anchor, which pairs the section across variants, stays \
the same.\n\n\
When you change what a document says, change both variants: write the one you edited, then its translation with \
write_doc, listing the translated section anchors in `resolves`.\n\n\
To translate changes made by others: call get_translation_guide once per project, take sections from \
get_sync_queue that have no current proposal, read each with get_sync_item, and submit the full translated \
variant with propose_translation. A person reviews and accepts proposals. Use resolve_sync only when an edit \
does not change meaning (typos, formatting) so no translation is needed. A document with one variant only is \
not in the sync queue; get_untranslated lists them, and proposing the missing variant starts its sync.\n\n\
Diagram fences (```mermaid) are shared by both variants, not translated: copy them verbatim. Editing a \
diagram's contents does not mark the other variant stale, and a human-variant edit is carried over to an \
identical copy in the AI variant automatically (write responses then return followed_ai_hash, the AI \
variant's new content_hash). Adding or removing a diagram is a change to translate.\n\n\
Projects inherit documents from parent projects; {{include project:path#anchor}} transcludes content.\n\n\
Sections can declare the repository files they describe with an HTML comment on its own lines, \
`<!-- sources: path/file.rs, dir/, src/**/*.sql -->` (paths relative to the repository root). Before changing code, \
call context_for_paths with the files you will touch to read the sections that describe them. After changing code, \
call affected_sections with the changed paths and update the sections it returns. get_drift_queue lists bound \
sections whose files changed since they were last verified (or were never verified); `git diff \
<verified_revision> -- <changed paths>` shows what changed. Fix the section (then translate it as usual) or, if \
it is still accurate, call verify_sources. Drift is computed against file hashes reported with report_sources \
(git blob ids, e.g. from `git ls-files -s`), usually by CI.\n\n\
changes_since lists the documents changed since a cursor from a previous call or a timestamp, with the section \
anchors added, changed and removed per variant. Keep the returned cursor to continue from it next time.";

type ToolResult = Result<CallToolResult, McpError>;

#[derive(Clone)]
pub struct SolidateMcp {
    app: App,
    /// Fixed bearer credential (stdio). `None`: the HTTP layer authenticates each request and
    /// stores the [`Ctx`] in the request extensions.
    bearer: Option<Arc<str>>,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
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

fn parse_hash(h: Option<&str>) -> Result<Option<Hash>, AppError> {
    h.map(|h| {
        h.parse()
            .map_err(|_| AppError::Invalid("base_hash must be a 64-character hex hash".into()))
    })
    .transpose()
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
pub struct ChangesArgs {
    pub project: String,
    /// `cursor` from a previous changes_since response, or an RFC 3339 timestamp.
    /// Omit to get a cursor for the current point without changes.
    pub since: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Full-text query (web search syntax: quotes, OR, -exclude).
    pub query: String,
    /// Limit to one project.
    pub project: Option<String>,
    /// Limit to one variant: `human` or `ai`. Omit for both.
    pub variant: Option<String>,
    /// Maximum results (default 20, max 100).
    pub limit: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReadDocArgs {
    pub project: String,
    /// Document path, e.g. `design/auth`.
    pub path: String,
    /// `human` or `ai`. Default: `ai`, or `human` when no AI variant exists.
    pub variant: Option<String>,
    /// Return only the section with this anchor.
    pub section: Option<String>,
    /// With `section`: include its subsections (the following deeper headings).
    #[serde(default)]
    pub subsections: bool,
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
pub struct WriteSectionArgs {
    pub project: String,
    pub path: String,
    /// `human` (default) or `ai`.
    pub variant: Option<String>,
    /// Anchor of the section to replace. Empty `content` deletes it.
    pub anchor: Option<String>,
    /// Instead of `anchor`: insert a new section after this section and its subsections.
    /// Omit both to append at the end.
    pub after: Option<String>,
    /// With `anchor`: the section's subsections are replaced too.
    #[serde(default)]
    pub subsections: bool,
    /// Markdown of the section, starting with its heading line.
    pub content: String,
    /// `hash` of the section as last read (read_doc with `section` and the same
    /// `subsections`, without `expand`). Fails only if this section changed.
    pub section_hash: Option<String>,
    /// `content_hash` of the whole variant as last read. Fails if any part changed.
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
pub struct ProposeArgs {
    pub project: String,
    pub path: String,
    /// The variant being translated into: `human` or `ai`.
    pub variant: String,
    /// Full Markdown content of that variant with the translation applied.
    pub content: String,
    /// `content_hash` of that variant as last read (`human_head`/`ai_head` from
    /// get_sync_item). Omit only when the variant does not exist yet.
    pub base_hash: Option<String>,
    /// Note for the reviewer: what changed, and anything ambiguous in the source.
    pub message: Option<String>,
    /// Section anchors the translation covers. Omit for every section needing attention.
    #[serde(default)]
    pub resolves: Vec<String>,
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

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReportSourcesArgs {
    pub project: String,
    /// Commit id of the reported tree.
    pub revision: Option<String>,
    /// Path (relative to the repository root) → git blob object id.
    #[serde(default)]
    pub files: Files,
    /// Paths deleted since the previous report.
    #[serde(default)]
    pub removed: Vec<String>,
    /// `files` is the complete tree; files missing from it are removed.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AffectedArgs {
    pub project: String,
    /// Changed repository paths, relative to the repository root.
    pub paths: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ContextArgs {
    pub project: String,
    /// Repository paths you are about to change (or are reading), relative to the
    /// repository root.
    pub paths: Vec<String>,
    /// Preferred variant: `ai` (default) or `human`.
    pub variant: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TranslateQueuePrompt {
    /// Project slug.
    pub project: String,
    /// Maximum sections to translate in this session (default 10).
    pub limit: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct DocumentChangePrompt {
    /// Project slug.
    pub project: String,
    /// Changed repository paths, separated by commas or whitespace.
    pub paths: String,
    /// What the code change does.
    pub summary: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct VerifyArgs {
    pub project: String,
    pub path: String,
    /// Anchors of bound sections that still describe their sources.
    pub anchors: Vec<String>,
    /// The report revision you checked against (from get_drift_queue); the call
    /// fails if a newer report arrived since.
    pub revision: Option<String>,
}

impl SolidateMcp {
    /// Server authenticating every call with bearer credential `bearer` (stdio).
    pub fn with_bearer(app: App, bearer: impl Into<Arc<str>>) -> Self {
        Self {
            app,
            bearer: Some(bearer.into()),
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Server reading the [`Ctx`] placed in request extensions by [`http_service`].
    fn for_http(app: App) -> Self {
        Self {
            app,
            bearer: None,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    async fn ctx(&self, ext: &Extensions) -> Result<Ctx, AppError> {
        match &self.bearer {
            Some(b) => self.app.authenticate(Credential::Bearer(b)).await,
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

    #[tool(
        description = "Full-text search across documents. Each hit is one matching section: `anchor` and `section_hash` go straight to read_doc(section=anchor) and write_section(section_hash). `anchor` is null when only the document title or path matched. Filter with `variant: \"ai\"` for the dense reference variant."
    )]
    async fn search(&self, Parameters(a): Parameters<SearchArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let variant = match a.variant.as_deref() {
            None => None,
            v => Some(try_app!(parse_variant(v))),
        };
        let hits = try_app!(
            self.app
                .search(&ctx, &a.query, a.project.as_deref(), variant, a.limit.unwrap_or(20))
                .await
        );
        let out: Vec<Value> = hits
            .iter()
            .map(|h| {
                json!({
                    "project": h.project_slug, "path": h.path, "variant": h.variant,
                    "title": h.title, "anchor": h.anchor, "section_title": h.section_title,
                    "section_hash": h.section_hash, "snippet": h.snippet_text(),
                })
            })
            .collect();
        ok(json!(out))
    }

    #[tool(
        description = "Read one variant of a document, or one section of it (optionally with its subsections). Without `variant`, reads the AI variant, or the human variant when no AI variant exists; the response names the variant read. Returns content, content_hash (pass as base_hash when writing), and the section outline with semantic hashes. A section read returns that section's `hash` (pass as section_hash to write_section)."
    )]
    async fn read_doc(&self, Parameters(a): Parameters<ReadDocArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let (variant, e) = match a.variant.as_deref() {
            None => {
                let e = try_app!(self.app.expanded_doc(&ctx, &a.project, &path, Variant::Ai).await);
                if e.view.head.is_some() {
                    (Variant::Ai, e)
                } else {
                    let e = try_app!(self.app.expanded_doc(&ctx, &a.project, &path, Variant::Human).await);
                    (Variant::Human, e)
                }
            }
            v => {
                let variant = try_app!(parse_variant(v));
                (
                    variant,
                    try_app!(self.app.expanded_doc(&ctx, &a.project, &path, variant).await),
                )
            }
        };
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
            let target = SectionTarget::Section {
                anchor,
                subsections: a.subsections,
            };
            let body = match a.subsections {
                true => analysis.section_tree_source(anchor).unwrap_or_default(),
                false => s.body.clone(),
            };
            return ok(json!({
                "path": path.as_str(), "variant": variant, "content_hash": head.content_hash,
                "anchor": s.anchor, "title": s.title, "level": s.level,
                "hash": span_hash(&analysis, target), "content": body,
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
        description = "Write the full content of one document variant. Requires base_hash (the content_hash last read) unless creating the variant. Writing an inherited path creates an override in this project. When writing the translation of your own change, list the translated section anchors in `resolves` to mark them in sync. Creating a variant whose counterpart exists marks the sections present in both as in sync."
    )]
    async fn write_doc(&self, Parameters(a): Parameters<WriteDocArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let variant = try_app!(parse_variant(a.variant.as_deref()));
        let expect = try_app!(parse_hash(a.base_hash.as_deref())).map_or(Expect::Absent, Expect::Head);
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
            "followed_ai_hash": r.followed,
            "sync_pending": pending,
        }))
    }

    #[tool(
        description = "Replace, delete or insert one section of a document variant without resending the rest. Replace: `anchor` plus the section's new Markdown (heading included; empty content deletes it) and a precondition, section_hash (the section's `hash` from read_doc; other sections may change meanwhile) or base_hash (the variant's content_hash). Insert: `after` an anchor, or neither to append; preconditions optional. Rejected if the edit would change other sections' anchors (e.g. an unclosed code fence) or inserted text lacks a leading heading. Returns the new content_hash and the anchors written."
    )]
    async fn write_section(&self, Parameters(a): Parameters<WriteSectionArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let variant = try_app!(parse_variant(a.variant.as_deref()));
        let target = match (a.anchor.as_deref(), a.after.as_deref()) {
            (Some(anchor), None) => SectionTarget::Section {
                anchor,
                subsections: a.subsections,
            },
            (None, Some(after)) => SectionTarget::After(after),
            (None, None) => SectionTarget::End,
            (Some(_), Some(_)) => return fail(AppError::Invalid("pass either anchor or after".into())),
        };
        let base = try_app!(parse_hash(a.base_hash.as_deref()));
        let section_hash = try_app!(parse_hash(a.section_hash.as_deref()));
        if matches!(target, SectionTarget::Section { .. }) && base.is_none() && section_hash.is_none() {
            return fail(AppError::Invalid(
                "replacing a section requires section_hash or base_hash".into(),
            ));
        }
        let r = try_app!(
            self.app
                .put_section(
                    &ctx,
                    PutSection {
                        project: &a.project,
                        path: &path,
                        variant,
                        target,
                        content: &a.content,
                        expect: base.map_or(Expect::Any, Expect::Head),
                        section_hash,
                        message: a.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                        resolves: &a.resolves,
                    },
                )
                .await
        );
        let pending: Vec<Value> = r
            .put
            .sync
            .iter()
            .filter(|s| s.state.needs_attention())
            .map(|s| json!({ "anchor": s.anchor, "state": s.state, "stale_side": s.state.stale_side() }))
            .collect();
        ok(json!({
            "path": r.put.document.path,
            "variant": variant,
            "content_hash": r.put.revision.content_hash,
            "changed": r.put.created,
            "followed_ai_hash": r.put.followed,
            "anchors": r.anchors,
            "sync_pending": pending,
        }))
    }

    #[tool(
        description = "Sections awaiting translation across a project's own documents: one variant changed since the last sync. stale_side is the variant to translate into; null means both changed (conflict: reconcile both into one truth). `proposal` is set when a translation proposal already covers the section; skip those unless outdated."
    )]
    async fn get_sync_queue(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        ok(json!(try_app!(self.app.sync_queue(&ctx, &a.project).await)))
    }

    #[tool(
        description = "Sync-enabled documents of a project that have only one variant. `missing` is the variant to write; submit it with propose_translation (base_hash omitted). These documents are not in the sync queue until both variants exist."
    )]
    async fn get_untranslated(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        ok(json!(try_app!(self.app.untranslated(&ctx, &a.project).await)))
    }

    #[tool(
        description = "Everything needed to translate one section: both variants' current text, text at the last sync, diffs since then, and each variant's content_hash (human_head/ai_head) for base_hash."
    )]
    async fn get_sync_item(&self, Parameters(a): Parameters<SyncItemArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        ok(json!(try_app!(
            self.app.sync_item(&ctx, &a.project, &path, &a.anchor).await
        )))
    }

    #[tool(
        description = "Mark sections as in sync without editing: the change needs no translation (typo, formatting) or the variants already agree. Pass anchors, or paired=true for every pending section present in both variants."
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

    #[tool(
        description = "How the human and AI variants of this project's documents differ, and the rules for translating between them. Read before translating."
    )]
    async fn get_translation_guide(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let g = try_app!(self.app.translation_guide(&ctx, &a.project).await);
        ok(json!(g))
    }

    #[tool(
        description = "Submit a translation for review: the full content of the stale variant with the other variant's changes carried over. Replaces any open proposal for that variant. A person accepts or rejects it; accepting writes it and marks `resolves` in sync."
    )]
    async fn propose_translation(&self, Parameters(a): Parameters<ProposeArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        let variant = try_app!(parse_variant(Some(&a.variant)));
        let base = try_app!(parse_hash(a.base_hash.as_deref()));
        let p = try_app!(
            self.app
                .propose(
                    &ctx,
                    Propose {
                        project: &a.project,
                        path: &path,
                        variant,
                        content: &a.content,
                        base,
                        message: a.message.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                        resolves: &a.resolves,
                    },
                )
                .await
        );
        ok(json!({
            "proposal": p.proposal.id,
            "path": p.proposal.path,
            "variant": p.proposal.variant,
            "resolves": p.proposal.resolves,
        }))
    }

    #[tool(
        description = "Open translation proposals on a project's own documents, with each one's resolves, diff against the current variant, and whether it is outdated."
    )]
    async fn list_proposals(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let ps = try_app!(self.app.project_proposals(&ctx, &a.project).await);
        let out: Vec<Value> = ps
            .iter()
            .map(|p| {
                json!({
                    "proposal": p.proposal.id, "path": p.proposal.path, "variant": p.proposal.variant,
                    "resolves": p.proposal.resolves, "message": p.proposal.message,
                    "outdated": p.outdated, "diff": p.diff,
                    "created_at": p.proposal.created_at.format(&Rfc3339).ok(),
                })
            })
            .collect();
        ok(json!(out))
    }

    #[tool(
        description = "Report repository file hashes for drift tracking: files maps path → git blob id (as printed by `git ls-files -s`). Partial by default (upsert files, delete removed); replace=true makes files the complete tree."
    )]
    async fn report_sources(&self, Parameters(a): Parameters<ReportSourcesArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let s = try_app!(
            self.app
                .report_sources(
                    &ctx,
                    &a.project,
                    SourceReport {
                        revision: a.revision.as_deref(),
                        files: &a.files,
                        removed: &a.removed,
                        replace: a.replace,
                    },
                )
                .await
        );
        ok(json!(s))
    }

    #[tool(
        description = "Documentation for code: sections bound to any of `paths` via `<!-- sources: -->`, with their content (AI variant unless `variant` says otherwise), `hash` (section_hash for write_section), `content_hash`, and `links` to related documents such as decision records. Call before changing code to learn its design and constraints; needs no source report."
    )]
    async fn context_for_paths(&self, Parameters(a): Parameters<ContextArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let prefer = match a.variant.as_deref() {
            None => Variant::Ai,
            v => try_app!(parse_variant(v)),
        };
        ok(json!(try_app!(
            self.app.section_context(&ctx, &a.project, &a.paths, prefer).await
        )))
    }

    #[tool(
        description = "Documents changed since a cursor or RFC 3339 timestamp: per variant, the section anchors added, changed and removed, the authors and revision messages; plus created and deleted documents. Store the returned cursor and pass it as since next time."
    )]
    async fn changes_since(&self, Parameters(a): Parameters<ChangesArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        ok(json!(try_app!(
            self.app.changes(&ctx, &a.project, a.since.as_deref()).await
        )))
    }

    #[tool(
        description = "Sections whose declared source files changed since they were last verified (state changed, with changed/added/removed paths and verified_revision), were never verified (unverified), or have patterns matching no reported file (missing). Update the section or call verify_sources."
    )]
    async fn get_drift_queue(&self, Parameters(a): Parameters<ProjectArg>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        ok(json!(try_app!(self.app.drift_queue(&ctx, &a.project).await)))
    }

    #[tool(
        description = "Sections whose source patterns match any of the given repository paths. Call after changing code to find the documentation to update."
    )]
    async fn affected_sections(&self, Parameters(a): Parameters<AffectedArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        ok(json!(try_app!(
            self.app.affected_sections(&ctx, &a.project, &a.paths).await
        )))
    }

    #[tool(
        description = "Confirm that sections still describe their source files as currently reported, clearing their drift. Returns the document's drift status."
    )]
    async fn verify_sources(&self, Parameters(a): Parameters<VerifyArgs>, ext: Extensions) -> ToolResult {
        let ctx = try_app!(self.ctx(&ext).await);
        let path = try_app!(parse_path(&a.path));
        ok(json!(try_app!(
            self.app
                .verify_sources(&ctx, &a.project, &path, &a.anchors, a.revision.as_deref())
                .await
        )))
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

fn user_prompt(description: &str, text: String) -> GetPromptResult {
    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)]).with_description(description)
}

/// Splits a prompt argument listing paths by commas and whitespace.
fn split_paths(paths: &str) -> Vec<&str> {
    paths
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Prompts are workflow templates: they name the tools to call and in which order,
/// and read no data themselves.
#[prompt_router]
impl SolidateMcp {
    #[prompt(
        name = "translate-queue",
        description = "Translate pending sections of a project's sync queue and submit them as proposals for review."
    )]
    async fn translate_queue(
        &self,
        Parameters(a): Parameters<TranslateQueuePrompt>,
    ) -> Result<GetPromptResult, McpError> {
        let limit = match a.limit.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            None => 10,
            Some(l) => l
                .parse::<u32>()
                .map_err(|_| McpError::invalid_params("limit must be a positive integer", None))?,
        };
        let p = &a.project;
        Ok(user_prompt(
            "Translate the sync queue",
            format!(
                "Translate pending documentation in Solidate project `{p}`, at most {limit} sections.\n\n\
                 1. Call get_translation_guide(project=\"{p}\") and follow its rules.\n\
                 2. Call get_sync_queue(project=\"{p}\"). Skip entries whose `proposal` is set and not outdated. \
                 An entry with stale_side null changed in both variants: do not translate it; list it in your \
                 summary for a person to reconcile.\n\
                 3. For each remaining section, call get_sync_item(project=\"{p}\", path, anchor). Carry the \
                 changes in the fresh variant over to the stale variant (stale_side). State the same facts; keep \
                 the heading text or explicit `{{#anchor}}` so the anchor stays paired.\n\
                 4. Call propose_translation with the full content of the stale variant, base_hash set to that \
                 variant's head from the sync item (human_head or ai_head), and `resolves` listing the anchors \
                 translated. Group all sections of one document into one proposal. In `message`, note anything \
                 ambiguous in the source.\n\
                 5. If a change does not alter meaning (typo, formatting), call resolve_sync for that anchor \
                 instead of proposing.\n\
                 6. If sections remain within the limit, call get_untranslated(project=\"{p}\") and, for documents \
                 without a current proposal, read the existing variant with read_doc and propose the missing one \
                 (base_hash omitted). Copy diagram fences verbatim.\n\n\
                 Finish with a list of proposals submitted, sections resolved, and conflicts left for review."
            ),
        ))
    }

    #[prompt(
        name = "fix-drift",
        description = "Work through a project's drift queue: update sections whose source files changed, or verify them."
    )]
    async fn fix_drift(&self, Parameters(a): Parameters<ProjectArg>) -> GetPromptResult {
        let p = &a.project;
        user_prompt(
            "Fix documentation drift",
            format!(
                "Resolve documentation drift in Solidate project `{p}`. Run this from a checkout of the \
                 repository the project documents.\n\n\
                 1. Call get_drift_queue(project=\"{p}\"). Note its `revision`.\n\
                 2. For each entry:\n\
                 \x20  - state `changed`: run `git diff <verified_revision> -- <changed paths>` and read the \
                 added paths.\n\
                 \x20  - state `unverified`: read the files matching the section's patterns.\n\
                 \x20  - `missing` patterns: find where the code moved and fix the `<!-- sources: -->` line, or \
                 remove the pattern.\n\
                 3. Read the section with read_doc(project=\"{p}\", path, section=anchor, variant=\"human\") and \
                 compare it with the code.\n\
                 4. If it is inaccurate, rewrite it with write_section(variant=\"human\"), passing the section's \
                 `hash` as section_hash. Then read the same section with variant=\"ai\" and rewrite it with \
                 write_section(variant=\"ai\"), `resolves` set to the anchor.\n\
                 5. If it is still accurate (or once updated), call verify_sources(project=\"{p}\", path, anchors, \
                 revision) with the revision from step 1.\n\n\
                 Finish with the sections rewritten and the sections verified unchanged."
            ),
        )
    }

    #[prompt(
        name = "document-change",
        description = "Update the documentation that describes a set of changed repository files."
    )]
    async fn document_change(
        &self,
        Parameters(a): Parameters<DocumentChangePrompt>,
    ) -> Result<GetPromptResult, McpError> {
        let paths = split_paths(&a.paths);
        if paths.is_empty() {
            return Err(McpError::invalid_params("paths must name at least one file", None));
        }
        let p = &a.project;
        let list = serde_json::to_string(&paths).expect("strings serialize");
        let summary = a
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("The change: {s}\n\n"))
            .unwrap_or_default();
        Ok(user_prompt(
            "Document a code change",
            format!(
                "Update the documentation in Solidate project `{p}` for a change to these repository files: \
                 {list}.\n\n{summary}\
                 1. Call context_for_paths(project=\"{p}\", paths={list}, variant=\"human\") to read the \
                 sections bound to these files. Follow `links` to decision records when the change touches a \
                 recorded decision.\n\
                 2. Compare each section with the changed code (`git diff` of the paths). Leave sections that are \
                 still accurate unchanged.\n\
                 3. Rewrite inaccurate sections with write_section(variant=\"human\"), passing the section's \
                 `hash` as section_hash. Keep the heading text and the `<!-- sources: -->` line; update the line \
                 if files were added, moved or removed.\n\
                 4. Apply the same change to the AI variant: read_doc(section=anchor, variant=\"ai\"), then \
                 write_section(variant=\"ai\") with `resolves` set to the anchors you changed. Follow \
                 get_translation_guide(project=\"{p}\") for the AI variant's style.\n\
                 5. If a new file has no documentation, use search to find the document it belongs in and add a \
                 section with write_section(after=anchor), including a `<!-- sources: -->` line.\n\
                 6. If source reports are in use, call verify_sources for the sections you checked.\n\n\
                 Finish with the sections changed and the sections checked but left as they were."
            ),
        ))
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for SolidateMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
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
