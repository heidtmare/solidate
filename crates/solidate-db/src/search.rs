//! Full-text search over document heads.

use crate::ids::*;
use crate::models::*;
use crate::{Result, TenantTx};

impl TenantTx {
    /// Searches live documents using `websearch_to_tsquery` syntax. Restricted to
    /// `project` when given.
    pub async fn search(&mut self, query: &str, project: Option<ProjectId>, limit: i64) -> Result<Vec<SearchHit>> {
        Ok(sqlx::query_as(
            "WITH q AS (SELECT websearch_to_tsquery('english', $1) AS q)
             SELECT d.id AS document_id, p.slug AS project_slug, d.path, h.title, h.variant,
                    ts_rank(h.search, q.q) AS rank,
                    ts_headline('english', b.content, q.q,
                        'StartSel=<mark>, StopSel=</mark>, MaxFragments=2, MaxWords=20, MinWords=5') AS snippet
             FROM heads h
             CROSS JOIN q
             JOIN documents d ON d.tenant_id = h.tenant_id AND d.id = h.document_id
             JOIN projects p ON p.tenant_id = d.tenant_id AND p.id = d.project_id
             JOIN blobs b ON b.tenant_id = h.tenant_id AND b.hash = h.content_hash
             WHERE h.search @@ q.q AND d.deleted_at IS NULL AND ($2::uuid IS NULL OR d.project_id = $2)
             ORDER BY rank DESC, d.path
             LIMIT $3",
        )
        .bind(query)
        .bind(project)
        .bind(limit)
        .fetch_all(self.conn())
        .await?)
    }
}
