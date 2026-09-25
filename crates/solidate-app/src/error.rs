use solidate_core::Hash;
use solidate_db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("authentication required")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    /// Precondition (`If-Match`) failed. `current` is the head's content hash.
    #[error("precondition failed")]
    PreconditionFailed { current: Option<Hash> },
    /// The token's request rate limit is exhausted.
    #[error("rate limit exceeded; retry in {retry_after_secs} s")]
    RateLimited { retry_after_secs: u64 },
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound => Self::NotFound,
            DbError::AlreadyExists(c) => Self::AlreadyExists(c),
            DbError::HeadMismatch { current } => Self::PreconditionFailed { current },
            DbError::Invalid(m) => Self::Invalid(m),
            DbError::Sqlx(e) => Self::Internal(e.to_string()),
        }
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;

pub(crate) fn invalid(msg: impl std::fmt::Display) -> AppError {
    AppError::Invalid(msg.to_string())
}
