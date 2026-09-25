//! Data not scoped to a tenant: tenants, users, external identities, sessions,
//! token lookup.

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

    /// Creates a user, with a password when `password_hash` is set.
    pub async fn create_user(&self, email: &str, name: &str, password_hash: Option<&str>) -> Result<User> {
        let mut tx = self.pool.begin().await?;
        let user: User = sqlx::query_as(
            "INSERT INTO users (id, email, name) VALUES ($1, $2, $3) RETURNING id, email, name, created_at, disabled_at",
        )
        .bind(UserId::new())
        .bind(email.trim())
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(hash) = password_hash {
            sqlx::query("INSERT INTO password_credentials (user_id, hash) VALUES ($1, $2)")
                .bind(user.id)
                .bind(hash)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(user)
    }

    pub async fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        Ok(
            sqlx::query_as("SELECT id, email, name, created_at, disabled_at FROM users WHERE lower(email) = lower($1)")
                .bind(email.trim())
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// The user with `email` and their password hash, if any.
    pub async fn password_login(&self, email: &str) -> Result<Option<PasswordLogin>> {
        Ok(sqlx::query_as(
            "SELECT u.id, u.email, u.name, u.created_at, u.disabled_at, p.hash FROM users u
             LEFT JOIN password_credentials p ON p.user_id = u.id
             WHERE lower(u.email) = lower($1)",
        )
        .bind(email.trim())
        .fetch_optional(&self.pool)
        .await?)
    }

    /// The user linked to the external identity `(issuer, subject)`.
    pub async fn identity_user(&self, issuer: &str, subject: &str) -> Result<Option<User>> {
        Ok(sqlx::query_as(
            "SELECT u.id, u.email, u.name, u.created_at, u.disabled_at FROM user_identities i
             JOIN users u ON u.id = i.user_id
             WHERE i.issuer = $1 AND i.subject = $2",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Links the external identity `(issuer, subject)` to `user`. `AlreadyExists`
    /// if the identity is linked to any user.
    pub async fn link_identity(&self, issuer: &str, subject: &str, user: UserId) -> Result<()> {
        sqlx::query("INSERT INTO user_identities (issuer, subject, user_id) VALUES ($1, $2, $3)")
            .bind(issuer)
            .bind(subject)
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Creates a user without a password, linked to the external identity
    /// `(issuer, subject)`.
    pub async fn create_identity_user(&self, email: &str, name: &str, issuer: &str, subject: &str) -> Result<User> {
        let mut tx = self.pool.begin().await?;
        let user: User = sqlx::query_as(
            "INSERT INTO users (id, email, name) VALUES ($1, $2, $3) RETURNING id, email, name, created_at, disabled_at",
        )
        .bind(UserId::new())
        .bind(email.trim())
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO user_identities (issuer, subject, user_id) VALUES ($1, $2, $3)")
            .bind(issuer)
            .bind(subject)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(user)
    }

    pub async fn user(&self, id: UserId) -> Result<Option<User>> {
        Ok(
            sqlx::query_as("SELECT id, email, name, created_at, disabled_at FROM users WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Sets or replaces the user's password.
    pub async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO password_credentials (user_id, hash) VALUES ($1, $2)
             ON CONFLICT (user_id) DO UPDATE SET hash = excluded.hash, updated_at = now()",
        )
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
            "SELECT u.id, u.email, u.name, u.created_at, u.disabled_at, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id
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
