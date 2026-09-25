//! Audit log: every mutating operation records an entry in its own transaction,
//! so an entry exists exactly when the change committed.

use serde_json::Value;
use solidate_db::{AuditEntry, AuditId, NewAudit, ProjectId, TenantTx};

use crate::App;
use crate::ctx::{Access, Actor, Ctx};
use crate::error::Result;

pub(crate) async fn record(
    tx: &mut TenantTx,
    ctx: &Ctx,
    action: &str,
    project: Option<ProjectId>,
    target: Option<&str>,
    detail: Value,
) -> Result<()> {
    let actor = match &ctx.actor {
        Actor::User { id, .. } => id.to_string(),
        Actor::Token { id, .. } => id.to_string(),
        Actor::System => "system".to_owned(),
    };
    tracing::info!(
        target: "solidate::audit",
        tenant = %ctx.tenant.slug,
        %actor,
        action,
        target = target.unwrap_or_default(),
        "audit"
    );
    tx.audit(NewAudit {
        actor: ctx.author(),
        action,
        project,
        target,
        detail,
    })
    .await?;
    Ok(())
}

impl App {
    /// Audit entries of the tenant, newest first. `before` continues a listing
    /// from the last entry's id.
    pub async fn audit_log(&self, ctx: &Ctx, limit: i64, before: Option<AuditId>) -> Result<Vec<AuditEntry>> {
        ctx.require(Access::Admin, None)?;
        Ok(self.tx(ctx).await?.audit_entries(limit.clamp(1, 500), before).await?)
    }
}
