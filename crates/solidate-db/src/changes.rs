//! Change feed rows (see migration `0007_changes.sql`).

use solidate_core::{Hash, Variant};
use time::OffsetDateTime;

use crate::ids::*;
use crate::{Result, TenantTx};

/// A feed window: transactions below `hi`, at or above `lo` when given, writing at or
/// after `since` when given. `hi` and `lo` are decimal transaction ids.
#[derive(Debug, Clone)]
pub struct ChangeWindow {
    pub hi: String,
    pub lo: Option<String>,
    pub since: Option<OffsetDateTime>,
}

/// A document with revisions, a deletion or an undelete in a window.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChangedDocument {
    pub id: DocumentId,
    pub project_id: ProjectId,
    pub path: String,
    pub title: Option<String>,
    /// Created in the window.
    pub created: bool,
    /// Deleted in the window, and not restored since.
    pub deleted: bool,
    /// Restored from deletion in the window, and not deleted since.
    pub restored: bool,
}

/// A revision in a window, with its author's display name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChangeRevision {
    pub id: RevisionId,
    pub document_id: DocumentId,
    pub variant: Variant,
    pub content_hash: Hash,
    pub parent_id: Option<RevisionId>,
    /// `user`, `token` or `system`.
    pub author_kind: String,
    /// User or token name.
    pub author_name: Option<String>,
    pub message: Option<String>,
    pub restored_from: Option<RevisionId>,
    pub created_at: OffsetDateTime,
}

/// Window predicate over binds `$1`..`$3` (`hi`, `lo`, `since`), given the row's
/// transaction id and time columns.
macro_rules! in_window {
    ($x:literal, $at:literal) => {
        concat!(
            "(",
            $x,
            " < $1::text::xid8 AND ($2::text IS NULL OR ",
            $x,
            " >= $2::text::xid8)
              AND ($3::timestamptz IS NULL OR ",
            $at,
            " >= $3))"
        )
    };
}

impl TenantTx {
    /// Upper bound for a window read now: transactions below it have finished.
    pub async fn change_horizon(&mut self) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT pg_snapshot_xmin(pg_current_snapshot())::text")
                .fetch_one(self.conn())
                .await?,
        )
    }

    /// Revisions of documents owned by `projects` in `w`, per document and variant in
    /// write order.
    pub async fn change_revisions(&mut self, projects: &[ProjectId], w: &ChangeWindow) -> Result<Vec<ChangeRevision>> {
        Ok(sqlx::query_as(concat!(
            "SELECT r.id, r.document_id, r.variant, r.content_hash, r.parent_id,
                    CASE WHEN r.author_user_id IS NOT NULL THEN 'user'
                         WHEN r.author_token_id IS NOT NULL THEN 'token' ELSE 'system' END AS author_kind,
                    coalesce(u.name, t.name) AS author_name, r.message, r.restored_from, r.created_at
             FROM revisions r
             JOIN documents d ON d.tenant_id = r.tenant_id AND d.id = r.document_id
             LEFT JOIN users u ON u.id = r.author_user_id
             LEFT JOIN api_tokens t ON t.tenant_id = r.tenant_id AND t.id = r.author_token_id
             WHERE d.project_id = ANY($4) AND ",
            in_window!("r.xid", "r.created_at"),
            " ORDER BY r.document_id, r.variant, r.created_at, r.id"
        ))
        .bind(&w.hi)
        .bind(&w.lo)
        .bind(w.since)
        .bind(projects)
        .fetch_all(self.conn())
        .await?)
    }

    /// `documents` plus the documents of `projects` deleted or restored in `w`.
    pub async fn changed_documents(
        &mut self,
        projects: &[ProjectId],
        documents: &[DocumentId],
        w: &ChangeWindow,
    ) -> Result<Vec<ChangedDocument>> {
        Ok(sqlx::query_as(concat!(
            "SELECT id, project_id, path, title,
                    ",
            in_window!("xid", "created_at"),
            " AS created,
                    coalesce(",
            in_window!("deleted_xid", "deleted_at"),
            ", false) AS deleted,
                    deleted_at IS NULL AND coalesce(",
            in_window!("restored_xid", "restored_at"),
            ", false) AS restored
             FROM documents
             WHERE id = ANY($5) OR (project_id = ANY($4) AND (",
            in_window!("deleted_xid", "deleted_at"),
            " OR (deleted_at IS NULL AND ",
            in_window!("restored_xid", "restored_at"),
            "))) ORDER BY path, id"
        ))
        .bind(&w.hi)
        .bind(&w.lo)
        .bind(w.since)
        .bind(projects)
        .bind(documents)
        .fetch_all(self.conn())
        .await?)
    }

    /// `(revision, anchor, hash)` of every section of `revisions`.
    pub async fn section_hashes(&mut self, revisions: &[RevisionId]) -> Result<Vec<(RevisionId, String, Hash)>> {
        Ok(sqlx::query_as(
            "SELECT revision_id, anchor, hash FROM sections WHERE revision_id = ANY($1) ORDER BY ordinal",
        )
        .bind(revisions)
        .fetch_all(self.conn())
        .await?)
    }
}
