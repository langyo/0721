use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::request::Permission;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    pub uploader: String,
    pub permission: Option<Permission>,
    pub created_at: DateTime<Utc>,

    pub name: String,
    pub hash: String,
    pub size: u64,
    pub mime: String,
}
