use anyhow::{anyhow, ensure, Context, Result};

use jsonwebtoken::{decode, Validation};

use super::{generate_token, Claims, JWT_SECRET};
use crate::functions::backend::user::*;
use _types::response::UserInfo;

pub async fn refresh(token: String) -> Result<UserInfo> {
    let token = decode::<Claims>(&token, &JWT_SECRET.decoding, &Validation::default())
        .context("Invalid token")?;

    let name = token.claims.name.clone();
    let user = get(name.clone()).await?.ok_or(anyhow!("User not found"))?;

    let iat = token.claims.iat;
    let updated_at = user.clone().updated_at
        - chrono::Duration::try_minutes(1).ok_or(anyhow!(
            "Failed to create token: Failed to subtract 1 minute from updated_at"
        ))?;
    ensure!(iat >= updated_at, "Token expired");

    let (token, updated_at) = generate_token(name.clone(), user.clone()).await?;

    Ok(UserInfo {
        token,
        name,
        permission: user.permission,
        updated_at,
    })
}
