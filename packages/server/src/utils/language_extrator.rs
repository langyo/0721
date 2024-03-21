use anyhow::Result;

use axum::{extract::FromRequestParts, http::request::Parts, response::Response};
use hyper::header::ACCEPT_LANGUAGE;

use _database::types::i18n::Language;

pub struct ExtractLanguageInfo(pub Language);

#[async_trait::async_trait]
impl<S> FromRequestParts<S> for ExtractLanguageInfo
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let language_raw = parts
            .headers
            .get(ACCEPT_LANGUAGE)
            .map(|header| header.to_str().map(|str| Some(str)).unwrap_or(None))
            .unwrap_or(None);

        if let Some(language_raw) = language_raw {
            let ret = match &language_raw[..2] {
                "zh" => match &language_raw[..5] {
                    "zh-HK" | "zh-TW" => Language::ZhHant,
                    _ => Language::ZhHans,
                },
                "ru" => Language::Ru,
                "fr" => Language::Fr,
                "de" => Language::De,
                "ja" => Language::Ja,
                "ko" => Language::Ko,
                "it" => Language::It,
                "en" | _ => Language::EnUs,
            };
            Ok(ExtractLanguageInfo(ret))
        } else {
            Ok(ExtractLanguageInfo(Language::EnUs))
        }
    }
}
