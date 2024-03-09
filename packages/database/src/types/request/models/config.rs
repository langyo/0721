use serde::{Deserialize, Serialize};

// TODO - Use static toml file instead

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub label: String,
    pub value: String,
}
