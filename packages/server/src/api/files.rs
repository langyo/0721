//! The multipart upload lane (`POST /api/v1/files`) — the binary
//! REST-whitelist twin of the RPC file methods (`crate::rpc`).
//!
//! This module owns ONLY the multipart transport: the validated,
//! authenticated upload chain itself is [`crate::api::upload_core`],
//! shared with the MCP `upload_file` tool.

use axum::extract::{Multipart, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;

use crate::api::auth::require_auth;
use crate::api::upload_core::{run_upload, UploadInput};
use crate::error::ApiError;
use crate::state::AppState;
use i0721_proto::FileDto;

/// Optional query of the upload endpoint (the legacy `?name=`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UploadQuery {
    /// Force the display name of the uploaded file.
    pub name: Option<String>,
    /// The destination folder (validated: exists, live, folder, owner).
    pub folder_id: Option<uuid::Uuid>,
}

/// `POST /api/v1/files` (multipart, field `file`)
///
/// Pipeline: size limit → SHA-256 → WebP convert → thumbnail → dedupe
/// probe → folder/quota/name checks → blob write → metadata row.
/// Identical content returns the existing record with `200`; a fresh
/// file answers `201`.
pub async fn upload(
    State(state): State<AppState>,
    Query(args): Query<UploadQuery>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
    mut multipart: Multipart,
) -> Result<axum::response::Response, ApiError> {
    let (_, user) = require_auth(&state, &headers, &uri).await?;

    let field = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::bad_request(format!("multipart read failed: {err}")))?
        .ok_or_else(|| ApiError::bad_request("no file field in the request"))?;
    let source_name = field.file_name().map(str::to_owned);
    let data = field
        .bytes()
        .await
        .map_err(|err| ApiError::bad_request(format!("multipart read failed: {err}")))?;

    let uploaded = run_upload(
        &state,
        &user,
        UploadInput {
            name: args.name,
            source_name,
            folder_id: args.folder_id,
            data,
        },
    )
    .await?;

    let status = if uploaded.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(FileDto::from(uploaded.record))).into_response())
}
