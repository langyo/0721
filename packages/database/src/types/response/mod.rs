mod auth;

pub use auth::*;

use serde::{Deserialize, Serialize};

use crate::models::user::Permission;

#[derive(PartialEq, Clone, Debug, Serialize, Deserialize)]
pub enum AuthInfo {
    None,
    User(UserInfo),
}

impl AuthInfo {
    pub fn is_user(&self) -> bool {
        match self {
            Self::User(_) => true,
            _ => false,
        }
    }

    pub fn is_admin(&self) -> bool {
        match self {
            Self::User(info) => {
                info.permission == Permission::Manager || info.permission == Permission::Root
            }
            _ => false,
        }
    }

    pub fn is_root(&self) -> bool {
        match self {
            Self::User(info) => info.permission == Permission::Root,
            _ => false,
        }
    }
}
