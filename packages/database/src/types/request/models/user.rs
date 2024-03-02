use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::user::Permission;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct User {
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub permission: Permission,
    pub password_hash: String,
    pub email: String,
}
