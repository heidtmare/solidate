//! Mapping service errors to HTTP responses.

use solidate_app::AppError;
use topcoat::router::error::{bad_request, forbidden, internal_server_error, not_found, redirect};

pub fn http(e: AppError) -> topcoat::Error {
    match e {
        AppError::NotFound => not_found().into(),
        AppError::Unauthorized => redirect("/login").into(),
        AppError::Forbidden => forbidden().into(),
        AppError::PreconditionFailed { .. } => bad_request("the document changed; reload and retry").into(),
        AppError::AlreadyExists(_) => bad_request("already exists").into(),
        AppError::Invalid(m) => bad_request(m).into(),
        e @ AppError::Internal(_) => internal_server_error(e).into(),
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
