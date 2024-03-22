use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::user::Permission;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    pub uploader: String,
    pub permission: Permission,
    pub created_at: DateTime<Utc>,

    pub hash: String,
    pub size: u64,
    pub mime: String,
}
