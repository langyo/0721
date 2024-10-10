use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{models::user::Model, request::Permission};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub token: String,
    pub name: String,
    pub email: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserBasicInfo {
    pub name: String,
    pub email: String,
    pub permission: Permission,
}

impl TryFrom<Model> for UserBasicInfo {
    type Error = anyhow::Error;

    fn try_from(user: Model) -> Result<Self> {
        Ok(Self {
            name: user.name,
            email: user.email,
            permission: serde_json::from_str(&user.permission)?,
        })
    }
}
