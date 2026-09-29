//! Restores: writing an earlier revision's content back as a new revision, and
//! undeleting documents.
//!
//! A restore of one variant also handles its counterpart when the document is in
//! sync:
//!
//! - Companion restore: for each section the restore changed, if the other
//!   variant's section is unchanged since its last reconciliation and
//!   `sync_base_log` records which text of it was in sync with the restored text,
//!   that text is written back too. A section the restore removed is removed from
//!   the other variant. Sections the other variant changed since, or whose paired
//!   text is unknown, are left for translation.
//! - Base restoration: after both writes, each affected section whose current pair
//!   of hashes was once recorded as in sync gets that base again.

use std::collections::HashMap;

use serde_json::json;
use solidate_core::diff::unified;
use solidate_core::edit::{SectionTarget, splice_section};
use solidate_core::sync::SectionSync;
use solidate_core::{DocPath, Hash, SyncBase, SyncState, Variant, analyze};
use solidate_db::{DeletedDocument, Document, DocumentId, Expect, Head, ProjectId, Revision, RevisionId, TenantTx};

use crate::App;
use crate::audit::record;
use crate::ctx::{Access, Ctx};
use crate::docs::{PutDoc, PutResult, WriteOpts};
use crate::error::{AppError, Result, invalid};
use crate::projects::project_by_slug;
use crate::sync::{doc_plan, own_doc, reconcile_anchors};

pub struct Restore<'a> {
    pub project: &'a str,
    pub path: &'a DocPath,
    pub revision: RevisionId,
    /// Precondition on the head of the revision's variant.
    pub expect: Expect,
    /// Defaults to "Restore revision <hash>".
    pub message: Option<&'a str>,
    /// Also restore the other variant's paired sections (see the module docs).
    pub companion: bool,
    /// Compute the result without writing it.
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// The write of the restored variant. `created` is `false` when the head
    /// already had the revision's content.
    pub put: PutResult,
    pub restored_from: Revision,
    /// Content hash of the restored variant's head before the restore; the
    /// precondition to confirm a dry run with.
    pub previous_hash: Hash,
    /// Unified diff of the restored variant, previous head → restored content.
    pub diff: String,
    pub companion: Option<CompanionWrite>,
    /// Sections of the other variant not restored: changed since their last
    /// reconciliation, or no paired text recorded.
    pub skipped: Vec<String>,
    /// Sections whose earlier sync base was recorded again.
    pub reconciled: Vec<String>,
    /// Sync plan after the restore. Empty when sync is disabled.
    pub sync: Vec<SectionSync>,
    /// `true` when nothing was committed.
    pub dry_run: bool,
}

/// The write of the other variant made by a companion restore.
#[derive(Debug, Clone)]
pub struct CompanionWrite {
    pub revision: Revision,
    /// Anchors of the sections restored, in document order of the restored variant.
    pub anchors: Vec<String>,
    /// Unified diff of the other variant, previous head → written content.
    pub diff: String,
}

/// A deleted document with the last head of each variant.
#[derive(Debug, Clone)]
pub struct DeletedDocView {
    pub document: DeletedDocument,
    pub human: Option<Head>,
    pub ai: Option<Head>,
}

fn by_anchor(plan: &[SectionSync]) -> HashMap<&str, &SectionSync> {
    plan.iter().map(|s| (s.anchor.as_str(), s)).collect()
}

/// Anchors whose `variant` hash differs between `before` and `after`, in `after`
/// order followed by anchors present only in `before`.
fn changed_anchors(before: &[SectionSync], after: &[SectionSync], variant: Variant) -> Vec<String> {
    let (b, a) = (by_anchor(before), by_anchor(after));
    let hash = |m: &HashMap<&str, &SectionSync>, k: &str| m.get(k).and_then(|s| s.hash(variant));
    after
        .iter()
        .map(|s| s.anchor.as_str())
        .chain(before.iter().map(|s| s.anchor.as_str()).filter(|k| !a.contains_key(k)))
        .filter(|k| hash(&b, k) != hash(&a, k))
        .map(str::to_owned)
        .collect()
}

/// Section text of `variant` with sync hash `hash`, from its newest revision
/// containing it.
async fn section_text(
    tx: &mut TenantTx,
    document: &Document,
    variant: Variant,
    anchor: &str,
    hash: Hash,
) -> Result<Option<String>> {
    let Some(content_hash) = tx.content_with_section(document.id, variant, anchor, hash).await? else {
        return Ok(None);
    };
    let Some(content) = tx.blob(content_hash).await? else {
        return Ok(None);
    };
    Ok(analyze(&content, None).section(anchor).map(|s| s.body.clone()))
}

/// Text of the `variant` side of `anchor` last recorded in sync with `other_hash`
/// on the other side, when it differs from `current`.
pub(crate) async fn paired_text(
    tx: &mut TenantTx,
    document: &Document,
    anchor: &str,
    variant: Variant,
    other_hash: Option<Hash>,
    current: Option<Hash>,
) -> Result<Option<String>> {
    let Some(other_hash) = other_hash else {
        return Ok(None);
    };
    let Some(base) = tx
        .sync_base_with(document.id, anchor, variant.other(), other_hash)
        .await?
    else {
        return Ok(None);
    };
    match base.side(variant) {
        Some(h) if Some(h) != current => section_text(tx, document, variant, anchor, h).await,
        _ => Ok(None),
    }
}

/// The other variant's content with each of `anchors` set to the text paired with
/// the restored variant `restored`. Returns the content, the anchors restored and
/// the anchors skipped.
async fn companion_content(
    tx: &mut TenantTx,
    document: &Document,
    restored: Variant,
    other_content: &str,
    before: &[SectionSync],
    after: &[SectionSync],
    anchors: &[String],
) -> Result<(String, Vec<String>, Vec<String>)> {
    let other = restored.other();
    let before = by_anchor(before);
    let after_map = by_anchor(after);
    let mut content = other_content.to_owned();
    let (mut done, mut skipped) = (Vec::new(), Vec::new());
    for anchor in anchors {
        if after_map
            .get(anchor.as_str())
            .is_some_and(|s| s.state == SyncState::InSync)
        {
            continue;
        }
        // The other side must be unchanged since its last reconciliation.
        let untouched = before
            .get(anchor.as_str())
            .is_none_or(|s| s.state == SyncState::InSync || s.state.stale_side() == Some(other));
        if !untouched {
            skipped.push(anchor.clone());
            continue;
        }
        let target = match after_map.get(anchor.as_str()).and_then(|s| s.hash(restored)) {
            Some(h) => match tx.sync_base_with(document.id, anchor, restored, h).await? {
                Some(base) => base.side(other),
                None => {
                    skipped.push(anchor.clone());
                    continue;
                }
            },
            // Removed by the restore.
            None => None,
        };
        let a = analyze(&content, None);
        let current = a.section(anchor);
        if target == current.map(|s| s.sync_hash) {
            continue;
        }
        let text = match target {
            Some(h) => match section_text(tx, document, other, anchor, h).await? {
                Some(t) => t,
                None => {
                    skipped.push(anchor.clone());
                    continue;
                }
            },
            None => String::new(),
        };
        let at = if current.is_some() {
            Some(SectionTarget::Section {
                anchor,
                subsections: false,
            })
        } else {
            // Insert after the nearest preceding section of the restored variant
            // that the other variant has.
            let pos = after.iter().position(|s| &s.anchor == anchor).unwrap_or(0);
            after[..pos]
                .iter()
                .rev()
                .find(|s| a.section(&s.anchor).is_some())
                .map(|s| SectionTarget::After(s.anchor.as_str()))
        };
        match at.map(|t| splice_section(&content, t, &text)) {
            Some(Ok(s)) => {
                content = s.content;
                done.push(anchor.clone());
            }
            _ => skipped.push(anchor.clone()),
        }
    }
    Ok((content, done, skipped))
}

impl App {
    /// Writes the content of an earlier revision back as the head of its variant,
    /// with a companion restore and base restoration (see the module docs). Only
    /// the owning project can restore a document.
    pub async fn restore(&self, ctx: &Ctx, req: Restore<'_>) -> Result<RestoreResult> {
        let mut tx = self.tx(ctx).await?;
        let document = own_doc(&mut tx, ctx, req.project, req.path, Access::Write).await?;
        let rev = tx
            .revision(req.revision)
            .await?
            .filter(|r| r.document_id == document.id)
            .ok_or(AppError::NotFound)?;
        let (restored, other) = (rev.variant, rev.variant.other());
        let content = tx.blob(rev.content_hash).await?.ok_or(AppError::NotFound)?;
        let previous = tx.head(document.id, restored).await?.ok_or(AppError::NotFound)?;
        let sync = document.sync_enabled;
        let before = if sync {
            doc_plan(&mut tx, document.id).await?.plan
        } else {
            Vec::new()
        };

        let message = req
            .message
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Restore revision {}", rev.content_hash.short()));
        let put = self
            .put_doc_with(
                &mut tx,
                ctx,
                PutDoc {
                    project: req.project,
                    path: req.path,
                    variant: restored,
                    content: &content,
                    expect: req.expect,
                    message: Some(&message),
                    resolves: &[],
                },
                WriteOpts {
                    restored_from: Some(rev.id),
                    ..WriteOpts::default()
                },
            )
            .await?;
        let diff = unified(&previous.content, &content, "current", "restored");

        let mut result = RestoreResult {
            sync: put.sync.clone(),
            put,
            restored_from: rev,
            previous_hash: previous.content_hash,
            diff,
            companion: None,
            skipped: Vec::new(),
            reconciled: Vec::new(),
            dry_run: req.dry_run,
        };
        if !result.put.created {
            return Ok(result);
        }

        let changed = changed_anchors(&before, &result.put.sync, restored);
        if sync && req.companion && !changed.is_empty() {
            // Read after the restore write, which may have carried diagrams over.
            if let Some(head) = tx.head(document.id, other).await? {
                let (content, anchors, skipped) = companion_content(
                    &mut tx,
                    &document,
                    restored,
                    &head.content,
                    &before,
                    &result.put.sync,
                    &changed,
                )
                .await?;
                result.skipped = skipped;
                if content.len() > self.config.max_doc_bytes {
                    return Err(invalid(format!("document exceeds {} bytes", self.config.max_doc_bytes)));
                }
                if !anchors.is_empty() && content != head.content {
                    let message = format!(
                        "Restore sections paired with {restored} revision {}",
                        result.restored_from.content_hash.short()
                    );
                    let put = self
                        .put_doc_with(
                            &mut tx,
                            ctx,
                            PutDoc {
                                project: req.project,
                                path: req.path,
                                variant: other,
                                content: &content,
                                expect: Expect::Head(head.content_hash),
                                message: Some(&message),
                                resolves: &[],
                            },
                            WriteOpts {
                                restored_from: None,
                                follow_diagrams: false,
                            },
                        )
                        .await?;
                    result.sync = put.sync;
                    result.companion = Some(CompanionWrite {
                        revision: put.revision,
                        anchors,
                        diff: unified(&head.content, &content, "current", "restored"),
                    });
                }
            }
        }

        if sync {
            let mut known = Vec::new();
            let touched = changed.iter().chain(result.companion.iter().flat_map(|c| &c.anchors));
            for anchor in touched {
                let Some(s) = result.sync.iter().find(|s| &s.anchor == anchor) else {
                    continue;
                };
                let base = SyncBase {
                    human: s.human,
                    ai: s.ai,
                };
                if s.state.needs_attention()
                    && !known.contains(anchor)
                    && tx.sync_base_known(document.id, anchor, base).await?
                {
                    known.push(anchor.clone());
                }
            }
            if !known.is_empty() {
                let plan = std::mem::take(&mut result.sync);
                result.sync = reconcile_anchors(&mut tx, document.id, plan, &known).await?;
            }
            result.reconciled = known;
        }

        let detail = json!({
            "variant": restored,
            "revision": result.put.revision.id,
            "restored_from": result.restored_from.id,
            "companion": result.companion.as_ref().map(|c| json!({
                "revision": c.revision.id,
                "anchors": c.anchors,
            })),
            "skipped": result.skipped,
            "reconciled": result.reconciled,
        });
        record(
            &mut tx,
            ctx,
            "doc.restore",
            Some(document.project_id),
            Some(req.path.as_str()),
            detail,
        )
        .await?;
        if !req.dry_run {
            tx.commit().await?;
        }
        Ok(result)
    }

    /// Deleted documents owned by `project`, newest deletion first.
    pub async fn deleted_docs(&self, ctx: &Ctx, project: &str) -> Result<Vec<DeletedDocument>> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        Ok(tx.deleted_documents(p.id).await?)
    }

    /// A deleted document owned by `project`, with its last content.
    pub async fn deleted_doc(&self, ctx: &Ctx, project: &str, id: DocumentId) -> Result<DeletedDocView> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let document = deleted_in(&mut tx, p.id, id).await?;
        Ok(DeletedDocView {
            human: tx.head(id, Variant::Human).await?,
            ai: tx.head(id, Variant::Ai).await?,
            document,
        })
    }

    /// Makes a deleted document owned by `project` live again at its path, with its
    /// revisions, sync bases and proposals as they were. Fails with `AlreadyExists`
    /// when a live document holds the path.
    pub async fn undelete_doc(&self, ctx: &Ctx, project: &str, id: DocumentId) -> Result<Document> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Write, Some(&p))?;
        let deleted = deleted_in(&mut tx, p.id, id).await?;
        let document = tx.undelete_document(id).await.map_err(|e| match AppError::from(e) {
            AppError::AlreadyExists(_) => AppError::AlreadyExists(format!(
                "a document exists at {}; delete it before restoring this one",
                deleted.path
            )),
            e => e,
        })?;
        let detail = json!({ "document": id, "deleted_at": deleted.deleted_at.unix_timestamp() });
        record(&mut tx, ctx, "doc.undelete", Some(p.id), Some(&document.path), detail).await?;
        tx.commit().await?;
        Ok(document)
    }
}

/// The deleted document `id` owned by `project`.
async fn deleted_in(tx: &mut TenantTx, project: ProjectId, id: DocumentId) -> Result<DeletedDocument> {
    tx.deleted_document(id)
        .await?
        .filter(|d| d.project_id == project)
        .ok_or(AppError::NotFound)
}
