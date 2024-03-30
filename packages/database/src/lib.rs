#[cfg(not(target_arch = "wasm32"))]
pub mod functions;
pub mod models;
pub mod types;

mod consts;
pub use consts::*;

use anyhow::Result;

#[cfg(not(target_arch = "wasm32"))]
pub async fn init() -> Result<()> {
    use chrono::Utc;
    use log::info;

    use crate::{
        functions::{backend::user::*, frontend::auth::generate_hash},
        models::user::{Model, Permission},
    };

    // If the user table is empty, create a default user
    if count().await? == 0 {
        let profile = Model {
            updated_at: Utc::now(),
            password_hash: generate_hash("admin")?,
            permission: Permission::Manager,
            email: "admin@localhost".to_string(),
        };
        set("admin", &profile).await?;
    }

    info!("Database is ready");

    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub async fn init() -> Result<()> {
    Ok(())
}
