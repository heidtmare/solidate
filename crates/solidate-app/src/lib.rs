//! Solidate use-case services. Front ends (web, MCP, CLI) authenticate the caller,
//! build a [`Ctx`], and call methods on [`App`]; authorization is enforced here.

mod audit;
mod auth;
mod ctx;
mod docs;
mod error;
mod projects;
mod ratelimit;
mod search;
mod sync;
pub mod telemetry;

pub use auth::{NewApiToken, TOKEN_PREFIX};
pub use ctx::{Access, Actor, Ctx};
pub use docs::{DocView, ExpandedDoc, PutDoc, PutResult, RenderedDoc, Tree, TreeEntry};
pub use error::{AppError, Result};
pub use ratelimit::RateLimit;
pub use sync::{DocSync, QueueEntry, SectionText, SyncItem};

pub use solidate_core as core;
pub use solidate_db as db;

use std::sync::Arc;

use ratelimit::RateLimiter;
use solidate_db::{Db, TenantTx};

#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum size of one document variant in bytes.
    pub max_doc_bytes: usize,
    /// Minimum password length.
    pub min_password_len: usize,
    /// Per-token API request limit. `None` disables limiting.
    pub rate_limit: Option<RateLimit>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_doc_bytes: 1 << 20,
            min_password_len: 8,
            rate_limit: Some(RateLimit {
                per_minute: 600,
                burst: 60,
            }),
        }
    }
}

impl Config {
    /// Defaults overridden by `SOLIDATE_RATE_LIMIT_PER_MIN` (`0` disables) and
    /// `SOLIDATE_RATE_LIMIT_BURST`.
    pub fn from_env() -> Result<Self, String> {
        fn var(name: &str) -> Result<Option<u32>, String> {
            match std::env::var(name) {
                Ok(v) => v.trim().parse().map(Some).map_err(|e| format!("{name}: {e}")),
                Err(_) => Ok(None),
            }
        }
        let mut c = Self::default();
        let default = c.rate_limit.expect("default rate limit");
        let per_minute = var("SOLIDATE_RATE_LIMIT_PER_MIN")?.unwrap_or(default.per_minute);
        let burst = var("SOLIDATE_RATE_LIMIT_BURST")?.unwrap_or(default.burst);
        c.rate_limit = (per_minute > 0).then_some(RateLimit { per_minute, burst });
        Ok(c)
    }
}

#[derive(Clone)]
pub struct App {
    db: Db,
    config: Config,
    limiter: Arc<RateLimiter>,
}

impl App {
    pub fn new(db: Db, config: Config) -> Self {
        let limiter = Arc::new(RateLimiter::new(config.rate_limit));
        Self { db, config, limiter }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    async fn tx(&self, ctx: &Ctx) -> Result<TenantTx> {
        Ok(self.db.tenant(ctx.tenant.id).await?)
    }
}
