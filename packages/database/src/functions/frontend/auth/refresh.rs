use anyhow::{anyhow, Result};

use jsonwebtoken::{decode, Validation};
use redb::ReadableTable as _;

use super::{generate_token, Claims, JWT_SECRET};
use crate::{models::*, types::response::UserInfo, DB_CONN};

pub async fn refresh(token: String) -> Result<UserInfo> {
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
    let updated_at_db = user.clone().updated_at - chrono::Duration::minutes(1);
    if iat < updated_at_db {
        return Err(anyhow!("Token expired"));
    }

    let (token, updated_at) = generate_token(name.clone(), user.clone()).await?;

    Ok(UserInfo {
        token,
        name,
        permission: user.permission,
        updated_at,
    })
}
