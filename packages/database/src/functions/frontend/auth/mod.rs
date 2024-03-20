mod login;
mod refresh;
mod verify;

pub use login::*;
pub use refresh::*;
pub use verify::*;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use jsonwebtoken::{encode, DecodingKey, EncodingKey, Header};

use crate::{functions::backend::user::*, models::user::Model};

struct Keys {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

static JWT_SECRET: Lazy<Keys> = Lazy::new(|| {
    let str = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    Keys {
        encoding: EncodingKey::from_secret(str.as_bytes()),
        decoding: DecodingKey::from_secret(str.as_bytes()),
    }
});

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Claims {
    name: String,
    #[serde(with = "jwt_numeric_date")]
    iat: DateTime<Utc>,
    #[serde(with = "jwt_numeric_date")]
    exp: DateTime<Utc>,
}

mod jwt_numeric_date {
    use chrono::{DateTime, TimeZone, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let timestamp = date.timestamp();
        serializer.serialize_i64(timestamp)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Utc.timestamp_opt(i64::deserialize(deserializer)?, 0)
            .single()
            .ok_or_else(|| serde::de::Error::custom("invalid Unix timestamp value"))
    }
}

pub async fn generate_token(name: String, user: Model) -> Result<(String, DateTime<Utc>)> {
    let now = chrono::Utc::now();
    let claims = Claims {
        name: name.clone(),
        iat: now,
        exp: now
            + chrono::Duration::try_days(15).ok_or(anyhow!(
                "Failed to create token: Failed to add 15 days to current time"
            ))?,
    };

    set(name, &user)
        .await
        .map_err(|err| anyhow!("Failed to update user: {}", err))?;

    Ok((
        encode(&Header::default(), &claims, &JWT_SECRET.encoding)
            .map_err(|err| anyhow!("Failed to encode token: {}", err))?,
        now,
    ))
}
