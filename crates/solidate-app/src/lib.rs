//! Solidate use-case services. Front ends (web, MCP, CLI) authenticate the caller,
//! build a [`Ctx`], and call methods on [`App`]; authorization is enforced here.

mod auth;
mod ctx;
mod docs;
mod error;
mod projects;
mod search;
mod sync;

pub use auth::{NewApiToken, TOKEN_PREFIX};
pub use ctx::{Access, Actor, Ctx};
pub use docs::{DocView, ExpandedDoc, PutDoc, PutResult, RenderedDoc, Tree, TreeEntry};
pub use error::{AppError, Result};
pub use sync::{DocSync, QueueEntry, SectionText, SyncItem};

pub use solidate_core as core;
pub use solidate_db as db;

use solidate_db::{Db, TenantTx};

#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum size of one document variant in bytes.
    pub max_doc_bytes: usize,
    /// Minimum password length.
    pub min_password_len: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_doc_bytes: 1 << 20,
            min_password_len: 8,
        }
    }
}

#[derive(Clone)]
pub struct App {
    db: Db,
    config: Config,
}

impl App {
    pub fn new(db: Db, config: Config) -> Self {
        Self { db, config }
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
