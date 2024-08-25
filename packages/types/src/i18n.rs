use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub header: config_item::Header,
    pub portal: config_item::Portal,
    pub images: config_item::Images,
    pub config: config_item::Config,
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
        pub config: String,

        pub login: String,
        pub logout: String,
        pub register: String,

        pub username: String,
        pub password: String,
        pub email: String,

        pub user: String,
        pub manager: String,
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

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub struct Images {
        pub total_count: String,
        pub load_more: String,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub struct Config {
        pub portal: config::Portal,
        pub router: config::Router,
        pub upload: config::Upload,
    }

    pub mod config {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub struct Portal {
            pub engine_version: String,
            pub title_suffix: String,
            pub footer_banner: String,
            pub language: String,
            pub timezone: String,
        }

        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub struct Router {
            pub media_entry_path: String,
            pub limit_referrer_host: String,
        }

        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub struct Upload {
            pub image_size_limit: String,
            pub webp_auto_convert: String,
            pub use_source_file_name: String,
        }
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
            Language::Ru => include_str!("../../../res/languages/ru.toml"),
            Language::Fr => include_str!("../../../res/languages/fr.toml"),
            Language::De => include_str!("../../../res/languages/de.toml"),
            Language::Ja => include_str!("../../../res/languages/ja.toml"),
            Language::Ko => include_str!("../../../res/languages/ko.toml"),
            Language::It => include_str!("../../../res/languages/it.toml"),
            Language::ZhHans => include_str!("../../../res/languages/zh_hans.toml"),
            Language::ZhHant => include_str!("../../../res/languages/zh_hant.toml"),
            Language::EnUs => include_str!("../../../res/languages/en_us.toml"),
        };
        toml::from_str(raw).context("Failed to parse toml")
    }
}
