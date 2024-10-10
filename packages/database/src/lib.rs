#[cfg(not(target_arch = "wasm32"))]
pub mod functions;
pub mod init;

pub use init::{InitRouteEnvParams, RouteEnv};

use anyhow::Result;
use log::info;

use sea_orm::{ConnectionTrait, Schema};

use crate::functions::backend::user::*;
use _types::{
    models,
    request::{Permission, RegisterParams},
};

macro_rules! create_table {
    ($db:path, $builder:path, $table:ident) => {
        $db.execute(
            $builder.build(
                Schema::new($builder)
                    .create_table_from_entity(models::$table::Entity)
                    .if_not_exists(),
            ),
        )
        .await?;
    };
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn init(env: RouteEnv) -> Result<()> {
    let db = env.sql.clone();
    let builder = env.sql.get_database_backend();

    create_table!(db, builder, user);

    // If the user table is empty, create a default user
    if count(env.clone()).await? == 0 {
        set(
            env.clone(),
            RegisterParams {
                name: Some("admin".to_string()),
                email: "admin@localhost".to_string(),
                password_raw: Some("admin".to_string()),
                permission: Some(Permission::Manager),
            },
        )
        .await?;
    }

    info!("Database is ready");

    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub async fn init() -> Result<()> {
    Ok(())
}
