use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginInfo {
    pub name: String,
    pub password_raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefreshInfo {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterParams {
    pub name: Option<String>,
    pub password_raw: Option<String>,
    pub email: String,
    pub permission: Option<Permission>,
}

#[derive(
    Clone, Debug, PartialEq, EnumIter, EnumString, Display, Deserialize, Serialize, Default,
)]
#[strum(serialize_all = "snake_case")]
pub enum Permission {
    #[default]
    User,
    Manager,
}

impl std::cmp::PartialOrd for Permission {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let self_num = match self {
            Permission::User => 1,
            Permission::Manager => 2,
        };
        let other_num = match other {
            Permission::User => 1,
            Permission::Manager => 2,
        };

        self_num.partial_cmp(&other_num)
    }
}
