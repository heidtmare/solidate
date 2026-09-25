//! Caller identity, projects, trees, root hashes, search, and `llms.txt`.

use serde::Serialize;
use solidate_app::core::{Hash, Variant};
use solidate_app::db::Project;
use solidate_app::{Actor, App, Ctx, Tree};
use topcoat::Result;
use topcoat::context::{Cx, app_context};
use topcoat::router::response::Response;
use topcoat::router::{StatusCode, path_param_segment, query_params, route};

use super::{ApiError, ApiResult, api_ctx, finish, json, not_modified, text, with_etag};
use crate::WebConfig;
use crate::auth::app;

#[derive(Serialize)]
struct ProjectOut<'a> {
    slug: &'a str,
    name: &'a str,
    /// Slug of the parent project.
    parent: Option<String>,
}

fn project_out<'a>(p: &'a Project, all: &[Project]) -> ProjectOut<'a> {
    ProjectOut {
        slug: &p.slug,
        name: &p.name,
        parent: p
            .parent_id
            .and_then(|id| all.iter().find(|x| x.id == id))
            .map(|x| x.slug.clone()),
    }
}

#[route(GET "/api/v1")]
async fn api_whoami(cx: &Cx) -> Result<Response> {
    finish(whoami_inner(cx).await)
}

async fn whoami_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    #[derive(Serialize)]
    struct Out<'a> {
        tenant: &'a str,
        tenant_name: &'a str,
        scopes: Vec<&'static str>,
        /// Project a restricted token is bound to.
        project: Option<String>,
    }
    let (scopes, project) = match &ctx.actor {
        Actor::Token { scopes, project, .. } => {
            let project = match project {
                Some(id) => app(cx)
                    .projects(&ctx)
                    .await?
                    .into_iter()
                    .find(|p| p.id == *id)
                    .map(|p| p.slug),
                None => None,
            };
            (scopes.iter().map(|s| s.as_str()).collect(), project)
        }
        _ => (Vec::new(), None),
    };
    Ok(json(
        StatusCode::OK,
        &Out {
            tenant: &ctx.tenant.slug,
            tenant_name: &ctx.tenant.name,
            scopes,
            project,
        },
    ))
}

#[route(GET "/api/v1/projects")]
async fn api_list(cx: &Cx) -> Result<Response> {
    finish(list_inner(cx).await)
}

async fn list_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let all = app(cx).projects(&ctx).await?;
    let out: Vec<_> = all.iter().map(|p| project_out(p, &all)).collect();
    Ok(json(StatusCode::OK, &out))
}

async fn tree_for(cx: &Cx, ctx: &Ctx) -> Result<Tree, ApiError> {
    Ok(app(cx).tree(ctx, path_param_segment(cx, "project")).await?)
}

#[route(GET "/api/v1/projects/{project}")]
async fn api_detail(cx: &Cx) -> Result<Response> {
    finish(detail_inner(cx).await)
}

async fn detail_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let slug = path_param_segment(cx, "project");
    let app = app(cx);
    let chain = app.project_chain(&ctx, slug).await?;
    let settings = app.effective_settings(&ctx, slug).await?;
    let tree = app.tree(&ctx, slug).await?;
    #[derive(Serialize)]
    struct Out<'a> {
        slug: &'a str,
        name: &'a str,
        /// Ancestors, nearest first.
        inherits: Vec<&'a str>,
        settings: serde_json::Value,
        root_hash: Hash,
        documents: usize,
    }
    Ok(json(
        StatusCode::OK,
        &Out {
            slug: &chain[0].slug,
            name: &chain[0].name,
            inherits: chain.iter().skip(1).map(|p| p.slug.as_str()).collect(),
            settings,
            root_hash: tree.root_hash,
            documents: tree.entries.len(),
        },
    ))
}

/// Effective documents with per-variant content hashes. ETag: the root hash.
#[route(GET "/api/v1/projects/{project}/tree")]
async fn api_tree(cx: &Cx) -> Result<Response> {
    finish(tree_inner(cx).await)
}

async fn tree_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let tree = tree_for(cx, &ctx).await?;
    if let Some(r) = not_modified(cx, Some(tree.root_hash))? {
        return Ok(r);
    }
    #[derive(Serialize)]
    struct Out<'a> {
        project: &'a str,
        root_hash: Hash,
        entries: &'a [solidate_app::TreeEntry],
    }
    let res = json(
        StatusCode::OK,
        &Out {
            project: &tree.project.slug,
            root_hash: tree.root_hash,
            entries: &tree.entries,
        },
    );
    Ok(with_etag(res, Some(tree.root_hash)))
}

/// Merkle root over all effective documents. Changes whenever any document the
/// project sees (including inherited ones) changes.
#[route(GET "/api/v1/projects/{project}/hash")]
async fn api_root_hash(cx: &Cx) -> Result<Response> {
    finish(root_hash_inner(cx).await)
}

async fn root_hash_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let tree = tree_for(cx, &ctx).await?;
    if let Some(r) = not_modified(cx, Some(tree.root_hash))? {
        return Ok(r);
    }
    #[derive(Serialize)]
    struct Out {
        root_hash: Hash,
    }
    Ok(with_etag(
        json(
            StatusCode::OK,
            &Out {
                root_hash: tree.root_hash,
            },
        ),
        Some(tree.root_hash),
    ))
}

#[query_params]
struct SearchQuery {
    q: Option<String>,
    project: Option<String>,
    limit: Option<i64>,
}

#[route(GET "/api/v1/search")]
async fn api_search(cx: &Cx) -> Result<Response> {
    finish(search_inner(cx).await)
}

async fn search_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let q = query_params::<SearchQuery>(cx).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let query = q.q.as_deref().unwrap_or("");
    let hits = app(cx)
        .search(&ctx, query, q.project.as_deref(), q.limit.unwrap_or(20))
        .await?;
    #[derive(Serialize)]
    struct Hit {
        project: String,
        path: String,
        variant: Variant,
        title: Option<String>,
        rank: f32,
        snippet: String,
    }
    let out: Vec<_> = hits
        .into_iter()
        .map(|h| Hit {
            snippet: h.snippet_text(),
            project: h.project_slug,
            path: h.path,
            variant: h.variant,
            title: h.title,
            rank: h.rank,
        })
        .collect();
    Ok(json(StatusCode::OK, &out))
}

/// `llms.txt` index of a project's documents, linking the raw Markdown of each
/// document's AI variant (human variant when no AI variant exists).
pub(crate) async fn llms_txt(app: &App, ctx: &Ctx, project: &str, public_url: &str) -> Result<String, ApiError> {
    let tree = app.tree(ctx, project).await?;
    let p = &tree.project;
    let mut out = format!(
        "# {}\n\n> Documentation of project `{}`. Each document is one ground truth written twice: a human variant (narrative) and an AI variant (dense, structured) that translate each other section by section. Links return raw Markdown; the AI variant is listed where one exists. API requests need `Authorization: Bearer <token>`.\n\n\
         When you change what a document says, update both variants: write the edited one, then its translation, listing the translated section anchors in `resolves`. \
         To translate changes made by others, read the translation guide ({public_url}/api/v1/projects/{}/translation-guide), take sections from the sync queue ({public_url}/api/v1/projects/{}/sync) that have no current proposal, and submit translations for review (`POST {public_url}/api/v1/projects/{}/propose/<path>`).\n\n## Documents\n\n",
        p.name, p.slug, p.slug, p.slug, p.slug
    );
    for e in &tree.entries {
        let variant = if e.ai.is_some() { Variant::Ai } else { Variant::Human };
        let title = e.title.as_deref().unwrap_or(&e.path);
        let inherited = if e.inherited {
            format!(", inherited from {}", e.owner)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "- [{title}]({public_url}/api/v1/projects/{}/docs/{}?variant={variant}&format=md): `{}` ({variant}{inherited})\n",
            p.slug, e.path, e.path
        ));
    }
    Ok(out)
}

#[route(GET "/api/v1/projects/{project}/llms.txt")]
async fn api_llms(cx: &Cx) -> Result<Response> {
    finish(llms_inner(cx).await)
}

async fn llms_inner(cx: &Cx) -> ApiResult {
    let ctx = api_ctx(cx).await?;
    let base = app_context::<WebConfig>(cx).public_url.as_deref().unwrap_or("");
    let body = llms_txt(app(cx), &ctx, path_param_segment(cx, "project"), base).await?;
    Ok(text("text/plain; charset=utf-8", body))
}
