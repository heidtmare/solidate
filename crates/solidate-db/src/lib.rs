//! PostgreSQL storage for Solidate.
//!
//! [`Db`] holds a pool whose connections run as role `solidate_app`, which is
//! subject to row-level security. Tenant-scoped data is reachable only through
//! [`TenantTx`], a transaction with `app.tenant_id` set; repository methods are
//! defined on it. Global data (tenants, users, sessions, token lookup) is accessed
//! through methods on [`Db`].
//!
//! Queries use runtime-checked `sqlx::query*` functions.

mod audit;
mod documents;
mod global;
mod ids;
mod models;
mod projects;
mod proposals;
mod search;
mod sync;
mod tokens;

use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Executor, PgPool, Postgres, Transaction};

pub use audit::NewAudit;
pub use documents::{Author, Expect, NewRevision};
pub use ids::*;
pub use models::*;
pub use proposals::NewProposal;
pub use tokens::NewToken;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("not found")]
    NotFound,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// The current head does not match the caller's expectation.
    #[error("head mismatch: current is {current:?}")]
    HeadMismatch { current: Option<solidate_core::Hash> },
    #[error("invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Sqlx(sqlx::Error),
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                Self::AlreadyExists(d.constraint().unwrap_or("unique").to_owned())
            }
            _ => Self::Sqlx(e),
        }
    }
}

pub type Result<T, E = DbError> = std::result::Result<T, E>;

#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connects with `url`; every connection switches to the `solidate_app` role.
    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with(
            PgPoolOptions::new().max_connections(16),
            PgConnectOptions::from_str(url)?,
        )
        .await
    }

    pub async fn connect_with(pool: PgPoolOptions, opts: PgConnectOptions) -> Result<Self> {
        let pool = pool
            .after_connect(|conn, _| {
                Box::pin(async move {
                    conn.execute("SET ROLE solidate_app").await?;
                    Ok(())
                })
            })
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }

    /// Runs pending migrations using an owner connection (not the app role).
    pub async fn migrate(url: &str) -> Result<()> {
        let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
        MIGRATOR.run(&pool).await.map_err(|e| DbError::Sqlx(e.into()))?;
        pool.close().await;
        Ok(())
    }

    /// Round-trips a trivial query (health checks).
    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Begins a transaction scoped to `tenant`.
    pub async fn tenant(&self, tenant: TenantId) -> Result<TenantTx> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant.to_string())
            .execute(&mut *tx)
            .await?;
        Ok(TenantTx { tx, tenant })
    }
}

/// A transaction with RLS scoped to one tenant. Dropped without [`commit`](Self::commit),
/// it rolls back.
pub struct TenantTx {
    tx: Transaction<'static, Postgres>,
    tenant: TenantId,
}

impl TenantTx {
    pub fn tenant(&self) -> TenantId {
        self.tenant
    }

    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await?;
        Ok(())
    }

    fn conn(&mut self) -> &mut sqlx::PgConnection {
        &mut self.tx
    }
}
