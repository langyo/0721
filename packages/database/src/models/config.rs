use serde::{Deserialize, Serialize};

use redb::TableDefinition;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Config {}

pub const TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
