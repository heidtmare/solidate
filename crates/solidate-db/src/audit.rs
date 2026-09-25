//! Audit log (append-only).

use crate::documents::Author;
use crate::ids::*;
use crate::models::*;
use crate::{Result, TenantTx};

pub struct NewAudit<'a> {
    pub actor: Author,
    /// Dotted action name, e.g. `doc.write`.
    pub action: &'a str,
    pub project: Option<ProjectId>,
    /// Operation-specific identifier: a document path, token id, user id, …
    pub target: Option<&'a str>,
    pub detail: serde_json::Value,
}

impl TenantTx {
    pub async fn audit(&mut self, e: NewAudit<'_>) -> Result<()> {
        let (kind, user, token) = match e.actor {
            Author::User(u) => ("user", Some(u), None),
            Author::Token(t) => ("token", None, Some(t)),
            Author::System => ("system", None, None),
        };
        sqlx::query(
            "INSERT INTO audit_log (id, actor_kind, actor_user_id, actor_token_id, action, project_id, target, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(AuditId::new())
        .bind(kind)
        .bind(user)
        .bind(token)
        .bind(e.action)
        .bind(e.project)
        .bind(e.target)
        .bind(e.detail)
        .execute(self.conn())
        .await?;
        Ok(())
    }

    /// Entries newest first, older than `before` when given.
    pub async fn audit_entries(&mut self, limit: i64, before: Option<AuditId>) -> Result<Vec<AuditEntry>> {
        Ok(sqlx::query_as(
            "SELECT a.id, a.at, a.actor_kind, a.actor_user_id, a.actor_token_id, a.action,
                    p.slug AS project_slug, a.target, a.detail
             FROM audit_log a LEFT JOIN projects p ON p.tenant_id = a.tenant_id AND p.id = a.project_id
             WHERE $2::uuid IS NULL OR a.id < $2
             ORDER BY a.id DESC LIMIT $1",
        )
        .bind(limit)
        .bind(before)
        .fetch_all(self.conn())
        .await?)
    }
}
