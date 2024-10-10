use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use yuuka::derive_struct;

derive_struct!(pub Config {
    header: {
        welcome: String,
        loading: String,

        portal: String,
        images: String,
        users: String,
        config: String,

        login: String,
        logout: String,
        register: String,

        username: String,
        password: String,
        email: String,

        user: String,
        manager: String,
    },
    portal: {
        upload: String,
        download: String,
        delete: String,

        progress: String,
        fail: String,
    },
    images: {
        total_count: String,
        load_more: String,
    },
    config: {
        portal: {
            engine_version: String,
            language: String,
            timezone: String,
            title_suffix: String,
            footer_banner: String,
        },
        router: {
            media_entry_path: String,
            limit_referrer_host: String,
        },
        upload: {
            image_size_limit: String,
            webp_auto_convert: String,
            use_source_file_name: String,
        },
    },
});

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
