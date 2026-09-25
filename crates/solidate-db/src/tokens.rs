//! API tokens (tenant-side management; pre-tenant lookup is in `global`).

use solidate_core::Scope;
use time::OffsetDateTime;

use crate::ids::*;
use crate::models::*;
use crate::{DbError, Result, TenantTx};

pub struct NewToken<'a> {
    pub name: &'a str,
    pub prefix: &'a str,
    pub secret_hash: &'a [u8],
    pub scopes: &'a [Scope],
    pub project: Option<ProjectId>,
    pub created_by: Option<UserId>,
    pub expires_at: Option<OffsetDateTime>,
}

impl TenantTx {
    pub async fn create_token(&mut self, t: NewToken<'_>) -> Result<ApiToken> {
        Ok(sqlx::query_as(
            "INSERT INTO api_tokens (id, name, prefix, secret_hash, scopes, project_id, created_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id, name, prefix, scopes, project_id, created_by, created_at, expires_at, last_used_at, revoked_at",
        )
        .bind(TokenId::new())
        .bind(t.name)
        .bind(t.prefix)
        .bind(t.secret_hash)
        .bind(t.scopes)
        .bind(t.project)
        .bind(t.created_by)
        .bind(t.expires_at)
        .fetch_one(self.conn())
        .await?)
    }

    pub async fn tokens(&mut self) -> Result<Vec<ApiToken>> {
        Ok(sqlx::query_as(
            "SELECT id, name, prefix, scopes, project_id, created_by, created_at, expires_at, last_used_at, revoked_at
             FROM api_tokens ORDER BY created_at DESC",
        )
        .fetch_all(self.conn())
        .await?)
    }

    pub async fn revoke_token(&mut self, id: TokenId) -> Result<()> {
        let n = sqlx::query("UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(self.conn())
            .await?
            .rows_affected();
        if n == 0 { Err(DbError::NotFound) } else { Ok(()) }
    }

    pub async fn touch_token(&mut self, id: TokenId) -> Result<()> {
        sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(self.conn())
            .await?;
        Ok(())
    }
}
