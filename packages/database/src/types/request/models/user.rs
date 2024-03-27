use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::user::{Model, Permission};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct User {
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub permission: Permission,
    pub password_hash: String,
    pub email: String,
}

impl From<User> for Model {
    fn from(user: User) -> Self {
        Self {
            updated_at: user.updated_at,
            permission: user.permission,
            password_hash: user.password_hash,
            email: user.email,
        }
    }
}
