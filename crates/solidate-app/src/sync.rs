//! Human/AI sync: per-document status, the project queue, item detail for
//! propagation, and resolution. State rules are in [`solidate_core::sync`].

use std::collections::HashMap;

use solidate_core::diff::unified;
use solidate_core::sync::{SectionSync, plan, reconcile};
use solidate_core::{DocPath, Hash, SyncState, Variant, analyze};
use solidate_db::{Document, DocumentId, Head, SectionRow, TenantTx};

use crate::App;
use crate::ctx::{Access, Ctx};
use crate::error::{AppError, Result, invalid};
use crate::projects::project_by_slug;

pub(crate) struct Plan {
    pub plan: Vec<SectionSync>,
    pub human: Option<Head>,
    pub ai: Option<Head>,
    pub human_sections: Vec<SectionRow>,
    pub ai_sections: Vec<SectionRow>,
}

fn pairs(rows: &[SectionRow]) -> Vec<(&str, Hash)> {
    rows.iter().map(|s| (s.anchor.as_str(), s.hash)).collect()
}

pub(crate) async fn doc_plan(tx: &mut TenantTx, document: DocumentId) -> Result<Plan> {
    let human = tx.head(document, Variant::Human).await?;
    let ai = tx.head(document, Variant::Ai).await?;
    let human_sections = match &human {
        Some(h) => tx.sections(h.revision_id).await?,
        None => Vec::new(),
    };
    let ai_sections = match &ai {
        Some(h) => tx.sections(h.revision_id).await?,
        None => Vec::new(),
    };
    let bases = tx.sync_bases(document).await?;
    let plan = plan(&pairs(&human_sections), &pairs(&ai_sections), &bases);
    Ok(Plan {
        plan,
        human,
        ai,
        human_sections,
        ai_sections,
    })
}

/// Records the current hashes of `anchors` as their sync base. Every anchor must be
/// in `plan`. Returns the updated plan.
pub(crate) async fn reconcile_anchors(
    tx: &mut TenantTx,
    document: DocumentId,
    mut plan: Vec<SectionSync>,
    anchors: &[String],
) -> Result<Vec<SectionSync>> {
    if let Some(unknown) = anchors.iter().find(|a| !plan.iter().any(|s| &s.anchor == *a)) {
        return Err(invalid(format!("unknown section anchor {unknown:?}")));
    }
    let refs: Vec<&str> = anchors.iter().map(String::as_str).collect();
    let bases = reconcile(&plan, &refs);
    tx.put_sync_bases(document, &bases).await?;
    for s in plan.iter_mut().filter(|s| refs.contains(&s.anchor.as_str())) {
        s.base = Some(s.reconciled());
        s.state = SyncState::InSync;
    }
    Ok(plan)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DocSync {
    pub document: Document,
    pub human_hash: Option<Hash>,
    pub ai_hash: Option<Hash>,
    pub sections: Vec<SectionStatus>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SectionStatus {
    pub anchor: String,
    pub title: String,
    pub state: SyncState,
    pub stale_side: Option<Variant>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueEntry {
    pub path: String,
    pub document_title: Option<String>,
    pub anchor: String,
    pub section_title: String,
    pub state: SyncState,
    pub stale_side: Option<Variant>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SectionText {
    pub title: String,
    pub body: String,
    pub hash: Hash,
}

/// What an actor needs to propagate one section.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncItem {
    pub path: String,
    pub anchor: String,
    pub state: SyncState,
    pub stale_side: Option<Variant>,
    pub human: Option<SectionText>,
    pub ai: Option<SectionText>,
    /// Section text at the last reconciliation, when still present in history.
    pub human_base: Option<String>,
    pub ai_base: Option<String>,
    /// Unified diff base → current, for each side that changed.
    pub human_diff: Option<String>,
    pub ai_diff: Option<String>,
    /// Content hashes to pass as `If-Match` when writing each variant.
    pub human_head: Option<Hash>,
    pub ai_head: Option<Hash>,
}

fn titles(p: &Plan) -> HashMap<&str, &str> {
    p.ai_sections
        .iter()
        .chain(&p.human_sections)
        .map(|s| (s.anchor.as_str(), s.title.as_str()))
        .collect()
}

async fn own_doc(tx: &mut TenantTx, ctx: &Ctx, project: &str, path: &DocPath, access: Access) -> Result<Document> {
    let p = project_by_slug(tx, project).await?;
    ctx.require(access, Some(&p))?;
    tx.document_by_path(p.id, path).await?.ok_or(AppError::NotFound)
}

impl App {
    pub async fn doc_sync(&self, ctx: &Ctx, project: &str, path: &DocPath) -> Result<DocSync> {
        let mut tx = self.tx(ctx).await?;
        let document = own_doc(&mut tx, ctx, project, path, Access::Read).await?;
        let p = doc_plan(&mut tx, document.id).await?;
        let titles = titles(&p);
        let sections = p
            .plan
            .iter()
            .map(|s| SectionStatus {
                anchor: s.anchor.clone(),
                title: titles.get(s.anchor.as_str()).copied().unwrap_or_default().to_owned(),
                state: s.state,
                stale_side: s.state.stale_side(),
            })
            .collect();
        Ok(DocSync {
            human_hash: p.human.as_ref().map(|h| h.content_hash),
            ai_hash: p.ai.as_ref().map(|h| h.content_hash),
            document,
            sections,
        })
    }

    /// Sections needing attention across `project`'s own sync-enabled documents.
    pub async fn sync_queue(&self, ctx: &Ctx, project: &str) -> Result<Vec<QueueEntry>> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let mut out = Vec::new();
        for d in tx.documents(p.id).await?.into_iter().filter(|d| d.sync_enabled) {
            let plan = doc_plan(&mut tx, d.id).await?;
            let titles = titles(&plan);
            for s in plan.plan.iter().filter(|s| s.state.needs_attention()) {
                out.push(QueueEntry {
                    path: d.path.clone(),
                    document_title: d.title.clone(),
                    anchor: s.anchor.clone(),
                    section_title: titles.get(s.anchor.as_str()).copied().unwrap_or_default().to_owned(),
                    state: s.state,
                    stale_side: s.state.stale_side(),
                });
            }
        }
        Ok(out)
    }

    pub async fn sync_item(&self, ctx: &Ctx, project: &str, path: &DocPath, anchor: &str) -> Result<SyncItem> {
        let mut tx = self.tx(ctx).await?;
        let document = own_doc(&mut tx, ctx, project, path, Access::Read).await?;
        let p = doc_plan(&mut tx, document.id).await?;
        let s = p
            .plan
            .iter()
            .find(|s| s.anchor == anchor)
            .ok_or(AppError::NotFound)?
            .clone();

        let current = |head: &Option<Head>| -> Option<SectionText> {
            let a = analyze(&head.as_ref()?.content, None);
            let sec = a.section(anchor)?;
            Some(SectionText {
                title: sec.title.clone(),
                body: sec.body.clone(),
                hash: sec.hash,
            })
        };
        let (human, ai) = (current(&p.human), current(&p.ai));

        let mut base_text = HashMap::new();
        for (variant, hash) in [
            (Variant::Human, s.base.and_then(|b| b.human)),
            (Variant::Ai, s.base.and_then(|b| b.ai)),
        ] {
            let Some(hash) = hash else { continue };
            let Some(content_hash) = tx.content_with_section(document.id, variant, anchor, hash).await? else {
                continue;
            };
            if let Some(content) = tx.blob(content_hash).await?
                && let Some(sec) = analyze(&content, None).section(anchor)
            {
                base_text.insert(variant, sec.body.clone());
            }
        }
        let human_base = base_text.remove(&Variant::Human);
        let ai_base = base_text.remove(&Variant::Ai);
        let diff = |base: &Option<String>, cur: &Option<SectionText>, changed: bool| {
            changed.then(|| {
                unified(
                    base.as_deref().unwrap_or(""),
                    cur.as_ref().map_or("", |c| c.body.as_str()),
                    "synced",
                    "current",
                )
            })
        };
        let human_changed = matches!(s.state, SyncState::HumanAhead | SyncState::Conflict);
        let ai_changed = matches!(s.state, SyncState::AiAhead | SyncState::Conflict);

        Ok(SyncItem {
            path: document.path,
            anchor: s.anchor,
            state: s.state,
            stale_side: s.state.stale_side(),
            human_diff: diff(&human_base, &human, human_changed),
            ai_diff: diff(&ai_base, &ai, ai_changed),
            human,
            ai,
            human_base,
            ai_base,
            human_head: p.human.map(|h| h.content_hash),
            ai_head: p.ai.map(|h| h.content_hash),
        })
    }

    /// Accepts the current content of `anchors` as in sync, without editing.
    pub async fn resolve_sync(&self, ctx: &Ctx, project: &str, path: &DocPath, anchors: &[String]) -> Result<DocSync> {
        {
            let mut tx = self.tx(ctx).await?;
            let document = own_doc(&mut tx, ctx, project, path, Access::Write).await?;
            let plan = doc_plan(&mut tx, document.id).await?.plan;
            reconcile_anchors(&mut tx, document.id, plan, anchors).await?;
            tx.commit().await?;
        }
        self.doc_sync(ctx, project, path).await
    }
}
