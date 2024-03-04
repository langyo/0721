use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Config {}
