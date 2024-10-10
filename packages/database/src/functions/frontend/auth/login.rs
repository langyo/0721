use anyhow::{anyhow, ensure, Result};

use bcrypt::{hash, verify as do_verify, DEFAULT_COST};

use super::generate_token;
use crate::{functions::backend::user::*, init::RouteEnv};
use _types::response::UserInfo;
use tairitsu_database::prelude::*;

pub fn verify_hash(input_raw: impl ToString, storage_hash: impl ToString) -> Result<bool> {
    Ok(do_verify(
        input_raw.to_string(),
        storage_hash.to_string().as_str(),
    )?)
}

pub fn generate_hash(password_raw: impl ToString) -> Result<String> {
    Ok(hash(password_raw.to_string(), DEFAULT_COST)?.to_string())
}

pub async fn login(env: RouteEnv, email: String, password_hash: String) -> Result<UserInfo> {
    let user = get(env.clone(), email.clone())
        .await?
        .ok_or(anyhow!("User not found"))?;

    ensure!(
        verify_hash(password_hash, user.password_hash.clone())?,
        "Wrong password"
    );

    let (token, updated_at) = generate_token(env.clone(), user.clone()).await?;
    env.kv
        .token_expired
        .set(user.email.clone(), updated_at.to_rfc3339())
        .await?;

    Ok(UserInfo {
        token,
        name: user.name,
        email: user.email,
        permission: serde_json::from_str(&user.permission)?,
        updated_at,
    })
}

#[cfg(test)]
mod test {
    #[test]
    fn generate_hash() {
        let hash = super::generate_hash("admin".to_string()).unwrap();
        println!("{}", hash);
    }
}
