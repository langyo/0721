use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    pub hash: String,
    pub update_at: DateTime<Utc>,
}
