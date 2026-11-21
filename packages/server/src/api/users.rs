//! `/api/v1/users` — admin-gated user management (list/create/delete).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use jiff::Timestamp;
use serde::Deserialize;
use uuid::Uuid;

use i0721_proto::{
    validate_display_name, validate_username, NewUser, PageArgs, PageResult, Role, UserDto,
};
use i0721_store::CreateUser;

use crate::api::auth::require_admin;
use crate::auth::{hash_password, watermark_key};
use crate::error::ApiError;
use crate::state::AppState;

/// The create-user request body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserRequest {
    /// Login name.
    pub username: String,
    /// Plaintext password (hashed before storage).
    pub password: String,
    /// Human-facing name; defaults to the username.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Role; defaults to member.
    #[serde(default)]
    pub role: Option<Role>,
}

/// `GET /api/v1/users?offset&limit`
pub async fn list(
    State(state): State<AppState>,
    Query(args): Query<PageArgs>,
    headers: HeaderMap,
) -> Result<Json<PageResult<UserDto>>, ApiError> {
    require_admin(&state, &headers).await?;
    let (offset, limit) = args.sanitize();
    let total = state.store.count_users().await?;
    let items = state
        .store
        .list_users(offset, limit)
        .await?
        .into_iter()
        .map(UserDto::from)
        .collect();
    Ok(Json(PageResult {
        total,
        offset,
        limit,
        items,
    }))
}

/// `POST /api/v1/users`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateUserRequest>,
) -> Result<axum::response::Response, ApiError> {
    require_admin(&state, &headers).await?;

    let params = NewUser {
        username: request.username,
        password: request.password,
        display_name: request.display_name,
        role: request.role,
    };
    validate_username(&params.username).map_err(|err| ApiError::bad_request(err.to_string()))?;
    if params.password.is_empty() || params.password.len() > 128 {
        return Err(ApiError::bad_request("password must be 1..=128 characters"));
    }
    let display_name = params
        .display_name
        .clone()
        .unwrap_or_else(|| params.username.clone());
    validate_display_name(&display_name).map_err(|err| ApiError::bad_request(err.to_string()))?;

    let password_hash = hash_password(&params.password)?;
    let record = state
        .store
        .create_user(CreateUser {
            username: params.username.clone(),
            display_name,
            role: params.role.unwrap_or(Role::Member),
            password_hash: Some(password_hash),
        })
        .await?;
    // No earlier tokens can exist for a brand-new user; recording the
    // watermark keeps the invariant explicit anyway.
    state
        .store
        .set_setting(
            &watermark_key(record.id),
            serde_json::Value::String(Timestamp::now().to_string()),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(UserDto::from(record))).into_response())
}

/// `DELETE /api/v1/users/{id}` — admins cannot delete themselves.
pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    if admin.id == id {
        return Err(ApiError::forbidden("cannot delete yourself"));
    }
    let removed = state.store.delete_user(id).await?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("user not found"))
    }
}
