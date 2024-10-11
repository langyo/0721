use anyhow::{anyhow, ensure, Context, Result};

use jsonwebtoken::{decode, Validation};

use super::{Claims, JWT_SECRET};
use crate::{functions::backend::user::*, init::RouteEnv};
use _types::response::UserInfo;
use tairitsu_database::prelude::*;

pub async fn verify(env: RouteEnv, token: String) -> Result<UserInfo> {
    let token_raw = token.clone();
    let token = decode::<Claims>(&token, &JWT_SECRET.decoding, &Validation::default())
        .context("Invalid token")?;

    let email = token.claims.email.clone();
    let user = get(env.clone(), email.clone())
        .await?
        .ok_or(anyhow!("User not found"))?;

    let iat = token.claims.iat;
    let updated_at = env
        .kv
        .token_expired
        .get(user.email.clone())
        .await?
        .ok_or(anyhow!("Token expired"))?;
    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&chrono::Utc);
    ensure!(
        iat >= updated_at - chrono::Duration::seconds(5),
        "Token expired"
    );

    Ok(UserInfo {
        token: token_raw,
        name: user.name,
        email: user.email,
        permission: serde_json::from_str(&user.permission)?,
        updated_at,
    })
}
