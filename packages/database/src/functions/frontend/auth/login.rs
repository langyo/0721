use anyhow::{anyhow, ensure, Result};

use bcrypt::{hash, verify as do_verify, DEFAULT_COST};

use super::generate_token;
use crate::functions::backend::user::*;
use _types::response::UserInfo;

pub fn verify_hash(input_raw: impl ToString, storage_hash: impl ToString) -> Result<bool> {
    Ok(do_verify(
        input_raw.to_string(),
        storage_hash.to_string().as_str(),
    )?)
}

pub fn generate_hash(password_raw: impl ToString) -> Result<String> {
    Ok(hash(password_raw.to_string(), DEFAULT_COST)?.to_string())
}

pub async fn login(name: String, password_hash: String) -> Result<UserInfo> {
    let user = get(name.clone()).await?.ok_or(anyhow!("User not found"))?;

    ensure!(
        verify_hash(password_hash, user.password_hash.clone())?,
        "Wrong password"
    );

    let (token, updated_at) = generate_token(name.clone(), user.clone()).await?;

    Ok(UserInfo {
        token,
        name,
        permission: user.permission,
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
