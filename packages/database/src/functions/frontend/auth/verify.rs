use anyhow::{anyhow, Result};

use jsonwebtoken::{decode, Validation};
use redb::ReadableTable as _;

use super::{Claims, JWT_SECRET};
use crate::{models::*, types::response::UserInfo, DB_CONN};

pub async fn verify(token: String) -> Result<UserInfo> {
    let token_raw = token.clone();
    let token = decode::<Claims>(&token, &JWT_SECRET.decoding, &Validation::default())
        .map_err(|e| anyhow!("Invalid token: {}", e))?;

    let name = token.claims.name.clone();
    let user = {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(user::TABLE)?;
        let raw = table.get(name.as_str())?.ok_or(anyhow!("User not found"))?;
        postcard::from_bytes::<user::Model>(raw.value())?
    };

    let iat = token.claims.iat;
    let updated_at = user.clone().updated_at;
    if iat < updated_at {
        return Err(anyhow!("Token expired"));
    }

    Ok(UserInfo {
        token: token_raw,
        name,
        permission: user.permission,
        updated_at,
    })
}
