//! Mapping service errors to HTTP responses.

use solidate_app::AppError;
use topcoat::router::error::{bad_request, forbidden, internal_server_error, not_found, redirect, too_many_requests};

pub fn http(e: AppError) -> topcoat::Error {
    match e {
        AppError::NotFound => not_found().into(),
        AppError::Unauthorized => redirect("/login").into(),
        AppError::Forbidden => forbidden().into(),
        AppError::PreconditionFailed { .. } => bad_request("the document changed; reload and retry").into(),
        AppError::AlreadyExists(_) => bad_request("already exists").into(),
        AppError::Invalid(m) => bad_request(m).into(),
        AppError::RateLimited { retry_after_secs } => too_many_requests(retry_after_secs).into(),
        AppError::Internal(m) => {
            tracing::error!(error = %m, "internal error");
            internal_server_error(AppError::Internal(m)).into()
        }
    }
}

pub trait OrHttp<T> {
    fn or_http(self) -> topcoat::Result<T>;
}

impl<T> OrHttp<T> for Result<T, AppError> {
    fn or_http(self) -> topcoat::Result<T> {
        self.map_err(http)
    }
}
