use anyhow::{anyhow, Result};

use bcrypt::{hash, verify as do_verify, DEFAULT_COST};
use redb::ReadableTable as _;

use super::generate_token;
use crate::{models::*, types::response::UserInfo, DB_CONN};

pub fn verify_hash(input_raw: impl ToString, storage_hash: impl ToString) -> Result<bool> {
    Ok(do_verify(
        input_raw.to_string(),
        storage_hash.to_string().as_str(),
    )?)
}

pub fn generate_hash(password_raw: impl ToString) -> Result<String> {
    Ok(hash(password_raw.to_string(), DEFAULT_COST)?.to_string())
}

pub async fn login(user_name: String, password_hash: String) -> Result<UserInfo> {
    let user = {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(user::TABLE)?;
        let raw = table
            .get(user_name.as_str())?
            .ok_or(anyhow!("User not found"))?;
        postcard::from_bytes::<user::Model>(raw.value())?
    };

    if !verify_hash(password_hash, user.password_hash.clone())? {
        return Err(anyhow!("Wrong password"));
    }

    let (token, updated_at) = generate_token(user_name.clone(), user.clone()).await?;

    Ok(UserInfo {
        token,
        name: user_name,
        permission: user.permission,
        updated_at,
    })
}

#[cfg(test)]
mod test {
    #[test]
    fn generate_hash() {
        let hash = super::generate_hash(format!("admin")).unwrap();
        println!("{}", hash);
    }
}
