//! Sync bases (see [`solidate_core::sync`]).

use std::collections::HashMap;

use solidate_core::{Hash, SyncBase};

use crate::ids::*;
use crate::{Result, TenantTx};

impl TenantTx {
    pub async fn sync_bases(&mut self, document: DocumentId) -> Result<HashMap<String, SyncBase>> {
        let rows: Vec<(String, Option<Hash>, Option<Hash>)> =
            sqlx::query_as("SELECT anchor, human_hash, ai_hash FROM sync_bases WHERE document_id = $1")
                .bind(document)
                .fetch_all(self.conn())
                .await?;
        Ok(rows
            .into_iter()
            .map(|(anchor, human, ai)| (anchor, SyncBase { human, ai }))
            .collect())
    }

    /// Upserts bases. A base with both sides `None` is deleted.
    pub async fn put_sync_bases(&mut self, document: DocumentId, bases: &[(String, SyncBase)]) -> Result<()> {
        let (gone, keep): (Vec<_>, Vec<_>) = bases.iter().partition(|(_, b)| b.human.is_none() && b.ai.is_none());
        sqlx::query("DELETE FROM sync_bases WHERE document_id = $1 AND anchor = ANY($2)")
            .bind(document)
            .bind(gone.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>())
            .execute(self.conn())
            .await?;
        sqlx::query(
            "INSERT INTO sync_bases (document_id, anchor, human_hash, ai_hash)
             SELECT $1, * FROM UNNEST($2::text[], $3::bytea[], $4::bytea[])
             ON CONFLICT (tenant_id, document_id, anchor)
             DO UPDATE SET human_hash = EXCLUDED.human_hash, ai_hash = EXCLUDED.ai_hash, updated_at = now()",
        )
        .bind(document)
        .bind(keep.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>())
        .bind(keep.iter().map(|(_, b)| b.human).collect::<Vec<_>>())
        .bind(keep.iter().map(|(_, b)| b.ai).collect::<Vec<_>>())
        .execute(self.conn())
        .await?;
        Ok(())
    }

    /// Removes bases whose anchors are not in `keep`.
    pub async fn prune_sync_bases(&mut self, document: DocumentId, keep: &[String]) -> Result<u64> {
        Ok(
            sqlx::query("DELETE FROM sync_bases WHERE document_id = $1 AND NOT (anchor = ANY($2))")
                .bind(document)
                .bind(keep)
                .execute(self.conn())
                .await?
                .rows_affected(),
        )
    }
}
