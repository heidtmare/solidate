//! Passwords, sessions, API tokens, and building a [`Ctx`].
//!
//! Request credentials enter through [`App::authenticate`], which picks the
//! scheme from the [`Credential`] and applies rate limiting. A new scheme adds a
//! `Credential` variant or a bearer format, and a function producing a [`Ctx`].
//!
//! API token format: `sol_<12 hex>_<64 hex>`. The `sol_<12 hex>` part is the stored
//! lookup prefix; the rest is the secret, stored as its BLAKE3 hash. Secrets carry
//! 256 bits of entropy, so a fast hash is sufficient.

use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use serde_json::json;
use solidate_core::{Role, Scope, Slug};
use solidate_db::{ApiToken, NewToken, ProjectId, Tenant, User, UserId};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

use crate::audit::record;
use crate::ctx::{Access, Ctx, Grant, Level, Principal};
use crate::error::{AppError, Result, invalid};
use crate::{App, Config};

pub const TOKEN_PREFIX: &str = "sol_";

/// A credential presented with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credential<'a> {
    /// Bearer token (`Authorization: Bearer …`, or `SOLIDATE_TOKEN` for stdio MCP).
    Bearer(&'a str),
}

impl<'a> Credential<'a> {
    /// `WWW-Authenticate` challenge for requests without a valid credential.
    pub const CHALLENGE: &'static str = "Bearer";

    /// Parses an `Authorization` header value. `None` for unsupported schemes.
    /// Scheme names are case-insensitive (RFC 9110 §11.1).
    pub fn from_authorization(value: &'a str) -> Option<Self> {
        let (scheme, param) = value.trim().split_once(' ')?;
        let param = param.trim_start();
        (scheme.eq_ignore_ascii_case("bearer") && !param.is_empty()).then_some(Self::Bearer(param))
    }
}

/// A newly created token. `secret` is the full token string; it is not stored.
#[derive(Debug, Clone)]
pub struct NewApiToken {
    pub token: ApiToken,
    pub secret: String,
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| AppError::Internal(format!("rng: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("argon2: {e}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// Hash verified when the user does not exist, so both paths cost the same.
fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| hash_password("dummy-password").expect("argon2 with default params"))
}

fn check_password_policy(config: &Config, password: &str) -> Result<()> {
    if password.chars().count() < config.min_password_len {
        return Err(invalid(format!(
            "password must be at least {} characters",
            config.min_password_len
        )));
    }
    Ok(())
}

impl App {
    pub async fn register_user(&self, email: &str, name: &str, password: &str) -> Result<User> {
        let email = email.trim();
        if !email.contains('@') || email.len() > 254 {
            return Err(invalid("invalid email"));
        }
        if name.trim().is_empty() {
            return Err(invalid("name is required"));
        }
        check_password_policy(&self.config, password)?;
        Ok(self
            .db
            .create_user(email, name.trim(), Some(&hash_password(password)?))
            .await?)
    }

    pub async fn set_password(&self, user: UserId, password: &str) -> Result<()> {
        check_password_policy(&self.config, password)?;
        Ok(self.db.set_password_hash(user, &hash_password(password)?).await?)
    }

    /// Verifies credentials. Fails with `Unauthorized` for unknown users, wrong
    /// passwords, users without a password and disabled accounts alike.
    pub async fn login(&self, email: &str, password: &str) -> Result<User> {
        match self.db.password_login(email).await? {
            Some(l) => {
                let hash = l.hash.as_deref();
                let verified = verify_password(password, hash.unwrap_or(dummy_hash())) && hash.is_some();
                let u = l.user;
                if verified && u.disabled_at.is_none() {
                    tracing::info!(target: "solidate::auth", user = %u.id, "login");
                    return Ok(u);
                }
                tracing::info!(target: "solidate::auth", user = %u.id, "login rejected");
                Err(AppError::Unauthorized)
            }
            None => {
                verify_password(password, dummy_hash());
                tracing::info!(target: "solidate::auth", "login rejected: unknown user");
                Err(AppError::Unauthorized)
            }
        }
    }

    pub async fn start_session(&self, token_hash: &[u8], user: UserId, expires_at: OffsetDateTime) -> Result<()> {
        Ok(self.db.create_session(token_hash, user, expires_at).await?)
    }

    pub async fn session_user(&self, token_hash: &[u8]) -> Result<Option<User>> {
        Ok(self.db.session(token_hash).await?.map(|s| s.user))
    }

    pub async fn extend_session(&self, token_hash: &[u8], expires_at: OffsetDateTime) -> Result<()> {
        Ok(self.db.extend_session(token_hash, expires_at).await?)
    }

    pub async fn end_session(&self, token_hash: &[u8]) -> Result<()> {
        Ok(self.db.delete_session(token_hash).await?)
    }

    pub async fn tenant(&self, slug: &str) -> Result<Tenant> {
        self.db.tenant_by_slug(slug).await?.ok_or(AppError::NotFound)
    }

    pub async fn create_tenant(&self, slug: &Slug, name: &str) -> Result<Tenant> {
        let tenant = self.db.create_tenant(slug, name).await?;
        let ctx = Ctx {
            tenant: tenant.clone(),
            principal: Principal::System,
            grant: Grant::unrestricted(),
        };
        let mut tx = self.tx(&ctx).await?;
        record(
            &mut tx,
            &ctx,
            "tenant.create",
            None,
            Some(&tenant.slug),
            json!({ "name": name }),
        )
        .await?;
        tx.commit().await?;
        Ok(tenant)
    }

    /// Context for `user` in tenant `slug`. `Forbidden` if the user is not a member.
    pub async fn user_ctx(&self, tenant_slug: &str, user: UserId) -> Result<Ctx> {
        let tenant = self.tenant(tenant_slug).await?;
        let mut tx = self.db.tenant(tenant.id).await?;
        let role = tx.role(user).await?.ok_or(AppError::Forbidden)?;
        Ok(Ctx {
            tenant,
            principal: Principal::User(user),
            grant: Grant {
                level: Level::Role(role),
                project: None,
            },
        })
    }

    pub async fn system_ctx(&self, tenant_slug: &str) -> Result<Ctx> {
        Ok(Ctx {
            tenant: self.tenant(tenant_slug).await?,
            principal: Principal::System,
            grant: Grant::unrestricted(),
        })
    }

    pub async fn set_member(&self, ctx: &Ctx, user: UserId, role: Role) -> Result<()> {
        ctx.require(Access::Admin, None)?;
        let mut tx = self.tx(ctx).await?;
        tx.set_membership(user, role).await?;
        let target = user.to_string();
        record(
            &mut tx,
            ctx,
            "member.set",
            None,
            Some(&target),
            json!({ "role": role.as_str() }),
        )
        .await?;
        Ok(tx.commit().await?)
    }

    pub async fn create_api_token(
        &self,
        ctx: &Ctx,
        name: &str,
        scopes: &[Scope],
        project: Option<ProjectId>,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<NewApiToken> {
        ctx.require(Access::Admin, None)?;
        if scopes.is_empty() {
            return Err(invalid("at least one scope is required"));
        }
        let prefix = format!("{TOKEN_PREFIX}{}", random_hex(6)?);
        let secret = random_hex(32)?;
        let mut tx = self.tx(ctx).await?;
        let token = tx
            .create_token(NewToken {
                name,
                prefix: &prefix,
                secret_hash: blake3::hash(secret.as_bytes()).as_bytes(),
                scopes,
                project,
                created_by: ctx.user_id(),
                expires_at,
            })
            .await?;
        let target = token.id.to_string();
        let detail = json!({
            "name": name,
            "prefix": token.prefix,
            "scopes": scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            "expires_at": expires_at,
        });
        record(&mut tx, ctx, "token.create", project, Some(&target), detail).await?;
        tx.commit().await?;
        Ok(NewApiToken {
            token,
            secret: format!("{prefix}_{secret}"),
        })
    }

    pub async fn api_tokens(&self, ctx: &Ctx) -> Result<Vec<ApiToken>> {
        ctx.require(Access::Admin, None)?;
        Ok(self.tx(ctx).await?.tokens().await?)
    }

    pub async fn revoke_api_token(&self, ctx: &Ctx, id: solidate_db::TokenId) -> Result<()> {
        ctx.require(Access::Admin, None)?;
        let mut tx = self.tx(ctx).await?;
        tx.revoke_token(id).await?;
        let target = id.to_string();
        record(&mut tx, ctx, "token.revoke", None, Some(&target), json!({})).await?;
        Ok(tx.commit().await?)
    }

    /// Authenticates a request credential. Each scheme checks the principal's
    /// rate limit after verifying the credential and before loading the tenant.
    pub async fn authenticate(&self, credential: Credential<'_>) -> Result<Ctx> {
        match credential {
            Credential::Bearer(raw) if raw.trim().starts_with(TOKEN_PREFIX) => self.api_token_ctx(raw).await,
            Credential::Bearer(_) => Err(AppError::Unauthorized),
        }
    }

    /// Shorthand for [`App::authenticate`] with a bearer token.
    pub async fn token_ctx(&self, raw: &str) -> Result<Ctx> {
        self.authenticate(Credential::Bearer(raw)).await
    }

    fn check_rate_limit(&self, principal: Principal) -> Result<()> {
        self.limiter.check(principal).map_err(|wait| {
            tracing::warn!(target: "solidate::auth", %principal, "rate limit exceeded");
            AppError::RateLimited {
                retry_after_secs: wait.as_secs_f64().ceil().max(1.0) as u64,
            }
        })
    }

    /// Verifies a raw `sol_…` token.
    async fn api_token_ctx(&self, raw: &str) -> Result<Ctx> {
        let raw = raw.trim();
        let rest = raw.strip_prefix(TOKEN_PREFIX).ok_or(AppError::Unauthorized)?;
        let (id, secret) = rest.split_once('_').ok_or(AppError::Unauthorized)?;
        let creds = self
            .db
            .token_by_prefix(&format!("{TOKEN_PREFIX}{id}"))
            .await?
            .ok_or(AppError::Unauthorized)?;
        let given = blake3::hash(secret.as_bytes());
        let matches: bool = given.as_bytes().ct_eq(creds.secret_hash.as_slice()).into();
        let now = OffsetDateTime::now_utc();
        if !matches || creds.revoked_at.is_some() || creds.expires_at.is_some_and(|e| e <= now) {
            return Err(AppError::Unauthorized);
        }
        let principal = Principal::Token(creds.id);
        self.check_rate_limit(principal)?;
        let mut tx = self.db.tenant(creds.tenant_id).await?;
        let tenant = tx.tenant_row().await?;
        tx.touch_token(creds.id).await?;
        tx.commit().await?;
        Ok(Ctx {
            tenant,
            principal,
            grant: Grant {
                level: Level::Scopes(creds.scopes),
                project: creds.project_id,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong", &h));
        assert!(!verify_password("x", "not a phc string"));
    }

    #[test]
    fn parses_authorization_header() {
        assert_eq!(
            Credential::from_authorization("Bearer sol_a_b"),
            Some(Credential::Bearer("sol_a_b"))
        );
        assert_eq!(
            Credential::from_authorization("bearer  sol_a_b "),
            Some(Credential::Bearer("sol_a_b"))
        );
        assert_eq!(Credential::from_authorization("Basic dXNlcjpwdw=="), None);
        assert_eq!(Credential::from_authorization("Bearer"), None);
        assert_eq!(Credential::from_authorization("Bearer "), None);
    }
}
