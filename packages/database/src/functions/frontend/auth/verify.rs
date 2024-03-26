use anyhow::{anyhow, ensure, Context, Result};

use jsonwebtoken::{decode, Validation};

use super::{Claims, JWT_SECRET};
use crate::{functions::backend::user::*, types::response::UserInfo};

pub async fn verify(token: String) -> Result<UserInfo> {
    let token_raw = token.clone();
    let token = decode::<Claims>(&token, &JWT_SECRET.decoding, &Validation::default())
        .context("Invalid token")?;

    let name = token.claims.name.clone();
    let user = get(name.clone()).await?.ok_or(anyhow!("User not found"))?;

    let iat = token.claims.iat;
    let updated_at = user.clone().updated_at;
    ensure!(iat >= updated_at, "Token expired");

    Ok(UserInfo {
        token: token_raw,
        name,
        permission: user.permission,
        updated_at,
    })
}
