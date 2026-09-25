//! Documents: inherited resolution, reads, writes, history, rendering, project trees.

use std::collections::{HashMap, HashSet};

use solidate_core::diff::unified;
use solidate_core::include::{DEFAULT_MAX_DEPTH, Dependency, expand_includes};
use solidate_core::markdown::OutlineEntry;
use solidate_core::{DocPath, Hash, LinkTarget, Merkle, Variant, analyze, render_html};
use solidate_db::{Backlink, Document, Expect, Head, NewRevision, Project, ProjectId, Revision, RevisionId, TenantTx};
use time::OffsetDateTime;

use crate::App;
use crate::ctx::{Access, Ctx};
use crate::error::{AppError, Result, invalid};
use crate::projects::project_by_slug;
use crate::sync::{doc_plan, reconcile_anchors};

/// A document as seen from `project`. `owner` differs from `project` when the
/// document is inherited from an ancestor.
#[derive(Debug, Clone)]
pub struct DocView {
    pub project: Project,
    pub owner: Project,
    pub document: Document,
    pub variant: Variant,
    /// `None` when this variant has not been written.
    pub head: Option<Head>,
}

impl DocView {
    pub fn inherited(&self) -> bool {
        self.owner.id != self.project.id
    }
}

pub struct PutDoc<'a> {
    pub project: &'a str,
    pub path: &'a DocPath,
    pub variant: Variant,
    pub content: &'a str,
    pub expect: Expect,
    pub message: Option<&'a str>,
    /// Anchors to mark in sync after the write (see [`crate::sync`]).
    pub resolves: &'a [String],
}

#[derive(Debug, Clone)]
pub struct PutResult {
    pub document: Document,
    pub revision: Revision,
    /// `false` when the content equalled the current head.
    pub created: bool,
    /// Sync plan after the write. Empty when sync is disabled for the document.
    pub sync: Vec<solidate_core::sync::SectionSync>,
}

#[derive(Debug, Clone)]
pub struct RenderedDoc {
    pub view: DocView,
    pub html: String,
    pub outline: Vec<OutlineEntry>,
    /// Content hash combined with the hashes of all included content.
    pub resolved_hash: Option<Hash>,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TreeEntry {
    pub path: String,
    pub title: Option<String>,
    /// Slug of the project that owns the document.
    pub owner: String,
    pub inherited: bool,
    pub sync_enabled: bool,
    pub human: Option<Hash>,
    pub ai: Option<Hash>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Tree {
    pub project: Project,
    /// Merkle root over `path@variant → content hash` of every effective document,
    /// including inherited ones.
    pub root_hash: Hash,
    pub entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone)]
pub struct RevisionDiff {
    pub revision: Revision,
    pub content: String,
    pub parent_content: Option<String>,
    /// Unified diff from the parent revision (or empty text) to this revision.
    pub diff: String,
}

/// Finds `path` in the first project of `chain` that defines it.
async fn resolve(tx: &mut TenantTx, chain: &[Project], path: &DocPath) -> Result<Option<(Project, Document)>> {
    for p in chain {
        if let Some(d) = tx.document_by_path(p.id, path).await? {
            return Ok(Some((p.clone(), d)));
        }
    }
    Ok(None)
}

async fn view(tx: &mut TenantTx, project: &Project, path: &DocPath, variant: Variant) -> Result<DocView> {
    let chain = tx.project_chain(project.id).await?;
    let (owner, document) = resolve(tx, &chain, path).await?.ok_or(AppError::NotFound)?;
    let head = tx.head(document.id, variant).await?;
    Ok(DocView {
        project: project.clone(),
        owner,
        document,
        variant,
        head,
    })
}

impl App {
    pub async fn get_doc(&self, ctx: &Ctx, project: &str, path: &DocPath, variant: Variant) -> Result<DocView> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        view(&mut tx, &p, path, variant).await
    }

    /// Writes one variant of a document in `project`, creating the document if needed.
    ///
    /// Writing to an inherited path creates an override in `project`. In that case an
    /// `Expect::Head` matching the inherited head is accepted.
    pub async fn put_doc(&self, ctx: &Ctx, req: PutDoc<'_>) -> Result<PutResult> {
        if req.content.len() > self.config.max_doc_bytes {
            return Err(invalid(format!("document exceeds {} bytes", self.config.max_doc_bytes)));
        }
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, req.project).await?;
        ctx.require(Access::Write, Some(&p))?;

        let (document, expect) = match tx.document_by_path(p.id, req.path).await? {
            Some(d) => (d, req.expect),
            None => {
                if let Expect::Head(h) = req.expect {
                    let chain = tx.project_chain(p.id).await?;
                    let inherited = match resolve(&mut tx, &chain, req.path).await? {
                        Some((_, d)) => tx.head(d.id, req.variant).await?.map(|h| h.content_hash),
                        None => None,
                    };
                    if inherited != Some(h) {
                        return Err(AppError::PreconditionFailed { current: None });
                    }
                }
                (tx.create_document(p.id, req.path).await?, Expect::Absent)
            }
        };

        let analysis = analyze(req.content, Some(req.path));
        let (revision, created) = tx
            .write_revision(NewRevision {
                document: document.id,
                variant: req.variant,
                content: req.content,
                analysis: &analysis,
                author: ctx.author(),
                message: req.message,
                expect,
            })
            .await?;

        let sync = if document.sync_enabled {
            let mut plan = doc_plan(&mut tx, document.id).await?.plan;
            if !req.resolves.is_empty() {
                plan = reconcile_anchors(&mut tx, document.id, plan, req.resolves).await?;
            }
            let keep: Vec<String> = plan.iter().map(|s| s.anchor.clone()).collect();
            tx.prune_sync_bases(document.id, &keep).await?;
            plan
        } else {
            Vec::new()
        };

        let document = tx.document(document.id).await?.ok_or(AppError::NotFound)?;
        tx.commit().await?;
        Ok(PutResult {
            document,
            revision,
            created,
            sync,
        })
    }

    /// Deletes the document owned by `project`. Inherited documents cannot be deleted
    /// from a descendant.
    pub async fn delete_doc(&self, ctx: &Ctx, project: &str, path: &DocPath) -> Result<()> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Write, Some(&p))?;
        let d = tx.document_by_path(p.id, path).await?.ok_or(AppError::NotFound)?;
        tx.delete_document(d.id).await?;
        Ok(tx.commit().await?)
    }

    pub async fn set_doc_sync(&self, ctx: &Ctx, project: &str, path: &DocPath, enabled: bool) -> Result<()> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Write, Some(&p))?;
        let d = tx.document_by_path(p.id, path).await?.ok_or(AppError::NotFound)?;
        tx.set_sync_enabled(d.id, enabled).await?;
        Ok(tx.commit().await?)
    }

    /// Effective documents of `project`: its own plus non-overridden inherited ones.
    pub async fn tree(&self, ctx: &Ctx, project: &str) -> Result<Tree> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let chain = tx.project_chain(p.id).await?;

        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for owner in &chain {
            for d in tx.documents(owner.id).await? {
                if seen.insert(d.path.clone()) {
                    entries.push(TreeEntry {
                        path: d.path,
                        title: d.title,
                        owner: owner.slug.clone(),
                        inherited: owner.id != p.id,
                        sync_enabled: d.sync_enabled,
                        human: d.human_hash,
                        ai: d.ai_hash,
                        updated_at: d.updated_at,
                    });
                }
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        let mut m = Merkle::new();
        for e in &entries {
            for (variant, hash) in [(Variant::Human, e.human), (Variant::Ai, e.ai)] {
                if let Some(h) = hash {
                    m.insert(format!("{}@{variant}", e.path), h);
                }
            }
        }
        Ok(Tree {
            project: p,
            root_hash: m.finish(),
            entries,
        })
    }

    /// Revisions of one variant, newest first.
    pub async fn history(
        &self,
        ctx: &Ctx,
        project: &str,
        path: &DocPath,
        variant: Variant,
        limit: i64,
    ) -> Result<(DocView, Vec<Revision>)> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let v = view(&mut tx, &p, path, variant).await?;
        let revs = tx.revisions(v.document.id, variant, limit.clamp(1, 500)).await?;
        Ok((v, revs))
    }

    pub async fn revision_diff(
        &self,
        ctx: &Ctx,
        project: &str,
        path: &DocPath,
        revision: RevisionId,
    ) -> Result<RevisionDiff> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let v = view(&mut tx, &p, path, Variant::Human).await?;
        let rev = tx
            .revision(revision)
            .await?
            .filter(|r| r.document_id == v.document.id)
            .ok_or(AppError::NotFound)?;
        let content = tx.blob(rev.content_hash).await?.ok_or(AppError::NotFound)?;
        let parent_content = match rev.parent_id {
            Some(pid) => match tx.revision(pid).await? {
                Some(parent) => tx.blob(parent.content_hash).await?,
                None => None,
            },
            None => None,
        };
        let diff = unified(
            parent_content.as_deref().unwrap_or(""),
            &content,
            "previous",
            "this revision",
        );
        Ok(RevisionDiff {
            revision: rev,
            content,
            parent_content,
            diff,
        })
    }

    pub async fn backlinks(&self, ctx: &Ctx, project: &str, path: &DocPath) -> Result<Vec<Backlink>> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let mut links = tx.backlinks(&p, path).await?;
        if ctx.restricted_project().is_some() {
            links.retain(|l| l.project_slug == p.slug);
        }
        Ok(links)
    }

    /// Renders a variant to HTML with includes expanded. `href` maps internal link
    /// targets to URLs; unqualified targets refer to `project`.
    pub async fn render_doc(
        &self,
        ctx: &Ctx,
        project: &str,
        path: &DocPath,
        variant: Variant,
        href: &dyn Fn(&LinkTarget) -> Option<String>,
    ) -> Result<RenderedDoc> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let v = view(&mut tx, &p, path, variant).await?;
        let Some(head) = &v.head else {
            return Ok(RenderedDoc {
                view: v,
                html: String::new(),
                outline: Vec::new(),
                resolved_hash: None,
                dependencies: Vec::new(),
            });
        };

        let allowed = match ctx.restricted_project() {
            Some(_) => Some(
                tx.project_chain(p.id)
                    .await?
                    .into_iter()
                    .map(|p| p.id)
                    .collect::<HashSet<_>>(),
            ),
            None => None,
        };
        let sources = collect_includes(&mut tx, &v.owner, variant, &head.content, allowed.as_ref()).await?;
        let owner = solidate_core::Slug::parse(&v.owner.slug).map_err(|e| AppError::Internal(e.to_string()))?;
        let expansion = expand_includes(
            &head.content,
            &owner,
            &mut |t: &LinkTarget| sources.get(&include_key(t)).cloned(),
            DEFAULT_MAX_DEPTH,
        );
        let rendered = render_html(&expansion.text, Some(path), href);
        let resolved_hash = Some(expansion.resolved_hash(head.content_hash));
        Ok(RenderedDoc {
            html: rendered.html,
            outline: rendered.outline,
            resolved_hash,
            dependencies: expansion.dependencies,
            view: v,
        })
    }
}

fn include_key(t: &LinkTarget) -> String {
    format!(
        "{}:{}",
        t.project.as_ref().map_or("", |p| p.as_str()),
        t.path.as_ref().map_or("", |p| p.as_str())
    )
}

/// Fetches the sources of all includes reachable from `md`, breadth-first up to the
/// expansion depth limit. Keys are `project:path`. Included documents resolve through
/// the target project's inheritance chain, preferring `variant` and falling back to
/// the human variant. Projects outside `allowed` (when given) are skipped.
async fn collect_includes(
    tx: &mut TenantTx,
    owner: &Project,
    variant: Variant,
    md: &str,
    allowed: Option<&HashSet<ProjectId>>,
) -> Result<HashMap<String, String>> {
    let mut sources = HashMap::new();
    let mut frontier = vec![(owner.slug.clone(), md.to_owned())];
    for _ in 0..DEFAULT_MAX_DEPTH {
        let mut next = Vec::new();
        for (context, text) in frontier {
            for mut target in analyze(&text, None).includes {
                if target.project.is_none() {
                    target.project = solidate_core::Slug::parse(&context).ok();
                }
                let key = include_key(&target);
                if sources.contains_key(&key) {
                    continue;
                }
                let (Some(slug), Some(path)) = (&target.project, &target.path) else {
                    continue;
                };
                let Some(project) = tx.project_by_slug(slug.as_str()).await? else {
                    continue;
                };
                if allowed.is_some_and(|a| !a.contains(&project.id)) {
                    continue;
                }
                let chain = tx.project_chain(project.id).await?;
                let Some((_, doc)) = resolve(tx, &chain, path).await? else {
                    continue;
                };
                let head = match tx.head(doc.id, variant).await? {
                    Some(h) => Some(h),
                    None => tx.head(doc.id, Variant::Human).await?,
                };
                if let Some(h) = head {
                    sources.insert(key, h.content.clone());
                    next.push((slug.to_string(), h.content));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(sources)
}
