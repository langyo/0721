use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::request::Permission;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub token: String,
    pub name: String,
    pub permission: Permission,
    pub updated_at: DateTime<Utc>,
}

impl UserInfo {
    pub fn is_user(&self) -> bool {
        self.permission == Permission::User || self.is_admin()
    }

    pub fn is_admin(&self) -> bool {
        self.permission == Permission::Manager
    }
}

impl std::cmp::PartialOrd for UserInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.permission.partial_cmp(&other.permission)
    }
}
