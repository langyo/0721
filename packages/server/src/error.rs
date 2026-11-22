//! The uniform error body — the ayane shape: `{"error": <reason>}` with
//! an HTTP status, for every API failure.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Serialize;

/// A uniform API error.
#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    /// Human-readable reason.
    pub error: String,
    #[serde(skip)]
    status: StatusCode,
}

impl ApiError {
    /// Build an error with an explicit status.
    #[must_use]
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            status,
        }
    }

    /// `400 Bad Request`.
    #[must_use]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// `401 Unauthorized`.
    #[must_use]
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    /// `403 Forbidden`.
    #[must_use]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// `404 Not Found`.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// `409 Conflict`.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    /// `413 Payload Too Large`.
    #[must_use]
    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, message)
    }

    /// `429 Too Many Requests`.
    #[must_use]
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }

    /// `500 Internal Server Error`.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self)).into_response()
    }
}

impl From<i0721_store::StoreError> for ApiError {
    fn from(err: i0721_store::StoreError) -> Self {
        match err {
            i0721_store::StoreError::Conflict(reason) => ApiError::conflict(reason),
            i0721_store::StoreError::Db(reason) => {
                tracing::error!(error = %reason, "store failure");
                ApiError::internal("internal storage failure")
            }
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        tracing::error!(error = %err, "blob failure");
        ApiError::internal("internal storage failure")
    }
}
