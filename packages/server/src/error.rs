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

    /// `502 Bad Gateway` (an upstream OAuth provider failed us).
    #[must_use]
    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }

    /// The HTTP status this error carries (the RPC lane maps it onto the
    /// JSON-RPC error-code family).
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Every error answer is uncacheable (I5 / R-S: a non-2xx response
        // carries `no-store` — including the bare error paths the media
        // lane's own faces do not wrap). MUT:P2B-APIERR-NOSTORE — dropping
        // this header reddens `every_api_error_answer_is_uncacheable`.
        let mut response = (self.status, Json(self)).into_response();
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
        response
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

#[cfg(test)]
mod no_store_tests {
    use super::ApiError;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    /// MUT:P2B-APIERR-NOSTORE — every ApiError answer carries `no-store`
    /// (R-S: non-2xx is uncacheable), whichever face produced it.
    #[test]
    fn every_api_error_answer_is_uncacheable() {
        let cases = [
            (StatusCode::NOT_FOUND, ApiError::not_found("probe")),
            (StatusCode::FORBIDDEN, ApiError::forbidden("probe")),
            (StatusCode::UNAUTHORIZED, ApiError::unauthorized("probe")),
            (StatusCode::BAD_REQUEST, ApiError::bad_request("probe")),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::internal("probe"),
            ),
        ];
        for (status, err) in cases {
            let response = err.into_response();
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers().get(axum::http::header::CACHE_CONTROL),
                Some(&axum::http::HeaderValue::from_static("no-store")),
                "{status} answers must carry no-store"
            );
        }
    }
}
