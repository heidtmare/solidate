//! Data not scoped to a tenant: tenants, users, sessions, token lookup.

use solidate_core::Slug;
use time::OffsetDateTime;

use crate::ids::*;
use crate::models::*;
use crate::{Db, Result};

impl Db {
    pub async fn create_tenant(&self, slug: &Slug, name: &str) -> Result<Tenant> {
        Ok(sqlx::query_as("SELECT * FROM create_tenant($1, $2)")
            .bind(slug.as_str())
            .bind(name)
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn tenant_by_slug(&self, slug: &str) -> Result<Option<Tenant>> {
        Ok(sqlx::query_as("SELECT * FROM tenant_by_slug($1)")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Tenants `user` belongs to, with the user's role in each.
    pub async fn user_tenants(&self, user: UserId) -> Result<Vec<TenantMembership>> {
        Ok(sqlx::query_as("SELECT * FROM user_tenants($1)")
            .bind(user)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn create_user(&self, email: &str, name: &str, password_hash: &str) -> Result<User> {
        Ok(
            sqlx::query_as("INSERT INTO users (id, email, name, password_hash) VALUES ($1, $2, $3, $4) RETURNING *")
                .bind(UserId::new())
                .bind(email.trim())
                .bind(name)
                .bind(password_hash)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        Ok(sqlx::query_as("SELECT * FROM users WHERE lower(email) = lower($1)")
            .bind(email.trim())
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn user(&self, id: UserId) -> Result<Option<User>> {
        Ok(sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<()> {
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(id)
            .bind(password_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_session(&self, token_hash: &[u8], user: UserId, expires_at: OffsetDateTime) -> Result<()> {
        sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(token_hash)
            .bind(user)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The session's user, if the session exists, is unexpired and the user is enabled.
    pub async fn session(&self, token_hash: &[u8]) -> Result<Option<SessionRecord>> {
        Ok(sqlx::query_as(
            "SELECT u.*, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn extend_session(&self, token_hash: &[u8], expires_at: OffsetDateTime) -> Result<()> {
        sqlx::query("UPDATE sessions SET expires_at = $2 WHERE token_hash = $1")
            .bind(token_hash)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_session(&self, token_hash: &[u8]) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_expired_sessions(&self) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    pub async fn token_by_prefix(&self, prefix: &str) -> Result<Option<TokenCredentials>> {
        Ok(sqlx::query_as("SELECT * FROM api_token_by_prefix($1)")
            .bind(prefix)
            .fetch_optional(&self.pool)
            .await?)
    }
}
