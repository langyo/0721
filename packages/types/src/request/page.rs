use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageArgs {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}
