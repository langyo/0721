use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub header: config_item::Header,
    pub portal: config_item::Portal,
}

pub mod config_item {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub struct Header {
        pub welcome: String,
        pub loading: String,

        pub portal: String,
        pub images: String,
        pub users: String,

        pub login: String,
        pub logout: String,
        pub register: String,

        pub username: String,
        pub password: String,
        pub email: String,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub struct Portal {
        pub upload: String,
        pub download: String,
        pub delete: String,

        pub progress: String,
        pub fail: String,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Ru,
    Fr,
    De,
    Ja,
    Ko,
    It,
    ZhHans,
    ZhHant,
    EnUs,
}

impl Language {
    pub fn to_config(self) -> Result<Config> {
        let raw = match self {
            Language::Ru => include_str!("../../../../res/languages/ru.toml"),
            Language::Fr => include_str!("../../../../res/languages/fr.toml"),
            Language::De => include_str!("../../../../res/languages/de.toml"),
            Language::Ja => include_str!("../../../../res/languages/ja.toml"),
            Language::Ko => include_str!("../../../../res/languages/ko.toml"),
            Language::It => include_str!("../../../../res/languages/it.toml"),
            Language::ZhHans => include_str!("../../../../res/languages/zh_hans.toml"),
            Language::ZhHant => include_str!("../../../../res/languages/zh_hant.toml"),
            Language::EnUs => include_str!("../../../../res/languages/en_us.toml"),
        };
        toml::from_str(raw).context("Failed to parse toml")
    }
}
