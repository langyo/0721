use serde::{Deserialize, Serialize};

use redb::TableDefinition;

use super::user::Permission;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    pub uploader: String,
    pub permission: Permission,

    pub hash: String,
    pub size: u64,
    pub mime: String,
}

pub const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("media");
