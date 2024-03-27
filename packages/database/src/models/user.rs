use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

#[derive(
    Clone, Debug, PartialEq, EnumIter, EnumString, Display, Deserialize, Serialize, Default,
)]
#[strum(serialize_all = "snake_case")]
pub enum Permission {
    #[default]
    Guest,
    User,
    Manager,
    Root,
}

impl std::cmp::PartialOrd for Permission {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let self_num = match self {
            Permission::Guest => 0,
            Permission::User => 1,
            Permission::Manager => 2,
            Permission::Root => 3,
        };
        let other_num = match other {
            Permission::Guest => 0,
            Permission::User => 1,
            Permission::Manager => 2,
            Permission::Root => 3,
        };

        self_num.partial_cmp(&other_num)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    pub updated_at: DateTime<Utc>,
    pub permission: Permission,
    pub password_hash: String,
    pub email: String,
}
