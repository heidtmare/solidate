//! HTTP API under `/api/v1`. Authentication: `Authorization: Bearer sol_…`; the
//! token determines the tenant. Responses are JSON unless noted. Errors have the
//! shape `{"error": {"code": "...", "message": "..."}}`.
//!
//! Content hashes are exposed as strong ETags (`"<hex>"`). Reads honour
//! `If-None-Match`; document writes require `If-Match: "<hash>"`, `If-Match: *`,
//! or `If-None-Match: *` (create only).

mod docs;
mod projects;
mod sync;

pub(crate) use projects::llms_txt;

use serde::Serialize;
use solidate_app::core::{DocPath, Hash, Variant};
use solidate_app::db::Expect;
use solidate_app::{AppError, Ctx};
use topcoat::context::Cx;
use topcoat::router::request::headers;
use topcoat::router::response::Response;
use topcoat::router::{Body, HeaderValue, StatusCode, header, path_param_segments};

use crate::auth::app;

pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    etag: Option<Hash>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            etag: None,
        }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Detail<'a> {
            code: &'a str,
            message: &'a str,
        }
        #[derive(Serialize)]
        struct Envelope<'a> {
            error: Detail<'a>,
        }
        let mut res = json(
            self.status,
            &Envelope {
                error: Detail {
                    code: self.code,
                    message: &self.message,
                },
            },
        );
        if self.status == StatusCode::UNAUTHORIZED {
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        if let Some(h) = self.etag {
            res.headers_mut().insert(header::ETAG, etag(h));
        }
        res
    }
}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", "not found"),
            AppError::Unauthorized => Self::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "a valid API token is required",
            ),
            AppError::Forbidden => Self::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "the token does not grant this access",
            ),
            AppError::PreconditionFailed { current } => Self {
                etag: current,
                ..Self::new(
                    StatusCode::PRECONDITION_FAILED,
                    "precondition_failed",
                    "the document changed; the current ETag is in the response headers",
                )
            },
            AppError::AlreadyExists(what) => Self::new(StatusCode::CONFLICT, "already_exists", what),
            AppError::Invalid(m) => Self::bad_request(m),
            AppError::Internal(m) => {
                eprintln!("api internal error: {m}");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
            }
        }
    }
}

pub(crate) type ApiResult = Result<Response, ApiError>;

/// Converts an API result into a route response; errors become JSON bodies.
pub(crate) fn finish(r: ApiResult) -> topcoat::Result<Response> {
    Ok(r.unwrap_or_else(ApiError::into_response))
}

pub(crate) async fn api_ctx(cx: &Cx) -> Result<Ctx, ApiError> {
    let token = headers(cx)
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;
    Ok(app(cx).token_ctx(token).await?)
}

pub(crate) fn json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    let body = serde_json::to_vec(value).expect("serializable response");
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}

pub(crate) fn text(content_type: &'static str, body: String) -> Response {
    let mut res = Response::new(Body::from(body));
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    res
}

pub(crate) fn etag(h: Hash) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{}\"", h.to_hex())).expect("hex is a valid header value")
}

pub(crate) fn with_etag(mut res: Response, h: Option<Hash>) -> Response {
    if let Some(h) = h {
        res.headers_mut().insert(header::ETAG, etag(h));
    }
    res
}

/// Entity tags listed in `name`: `*`, or hashes with optional `W/` prefixes.
enum Tags {
    Any,
    List(Vec<Hash>),
}

fn tags(cx: &Cx, name: header::HeaderName) -> Result<Option<Tags>, ApiError> {
    let Some(v) = headers(cx).get(&name) else {
        return Ok(None);
    };
    let v = v
        .to_str()
        .map_err(|_| ApiError::bad_request(format!("invalid {name}")))?
        .trim();
    if v == "*" {
        return Ok(Some(Tags::Any));
    }
    v.split(',')
        .map(|t| {
            let t = t.trim();
            t.strip_prefix("W/")
                .unwrap_or(t)
                .trim_matches('"')
                .parse::<Hash>()
                .map_err(|_| ApiError::bad_request(format!("invalid entity tag in {name}")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|l| Some(Tags::List(l)))
}

/// `304 Not Modified` when `If-None-Match` matches `current`.
pub(crate) fn not_modified(cx: &Cx, current: Option<Hash>) -> Result<Option<Response>, ApiError> {
    let Some(current) = current else { return Ok(None) };
    let hit = match tags(cx, header::IF_NONE_MATCH)? {
        Some(Tags::Any) => true,
        Some(Tags::List(l)) => l.contains(&current),
        None => false,
    };
    Ok(hit.then(|| {
        let mut res = with_etag(Response::new(Body::empty()), Some(current));
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        res
    }))
}

/// Write precondition from `If-Match` / `If-None-Match: *`.
pub(crate) fn write_precondition(cx: &Cx) -> Result<Expect, ApiError> {
    match (tags(cx, header::IF_MATCH)?, tags(cx, header::IF_NONE_MATCH)?) {
        (Some(Tags::Any), None) => Ok(Expect::Any),
        (Some(Tags::List(l)), None) if l.len() == 1 => Ok(Expect::Head(l[0])),
        (None, Some(Tags::Any)) => Ok(Expect::Absent),
        (None, None) => Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "precondition_required",
            "send If-Match with the current ETag, If-Match: *, or If-None-Match: * to create",
        )),
        _ => Err(ApiError::bad_request(
            "use either a single If-Match tag or If-None-Match: *",
        )),
    }
}

pub(crate) fn doc_path(cx: &Cx) -> Result<DocPath, ApiError> {
    let joined = path_param_segments(cx, "path").collect::<Vec<_>>().join("/");
    DocPath::parse(&joined).map_err(|e| ApiError::bad_request(e.to_string()))
}

pub(crate) fn parse_variant(v: Option<&str>) -> Result<Variant, ApiError> {
    match v {
        None | Some("human") => Ok(Variant::Human),
        Some("ai") => Ok(Variant::Ai),
        Some(other) => Err(ApiError::bad_request(format!(
            "unknown variant {other:?}; expected human or ai"
        ))),
    }
}

/// Whether the client prefers Markdown over JSON.
pub(crate) fn wants_markdown(cx: &Cx, format: Option<&str>) -> bool {
    match format {
        Some("md" | "markdown") => true,
        Some(_) => false,
        None => headers(cx)
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/markdown") && !a.contains("application/json")),
    }
}
