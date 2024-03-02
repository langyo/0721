use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginInfo {
    pub name: String,
    pub password_raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefreshInfo {
    pub token: String,
}
