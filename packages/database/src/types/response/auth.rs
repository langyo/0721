use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::user::Permission;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub token: String,
    pub name: String,
    pub permission: Permission,
    pub updated_at: DateTime<Utc>,
}
