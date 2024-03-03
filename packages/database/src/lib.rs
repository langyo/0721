pub mod functions;
pub mod models;
pub mod types;

mod consts;
pub use consts::*;

use anyhow::Result;

#[cfg(not(target_arch = "wasm32"))]
pub async fn init() -> Result<()> {
    use anyhow::anyhow;
    use chrono::Utc;
    use log::info;
    use redb::ReadableTable;

    use crate::{functions::frontend::auth::generate_hash, models::user::Permission};

    let db = redb::Database::create({
        let mut path = (*DATABASE_DIR).clone();
        let _ = tokio::fs::create_dir_all(&path).await;
        path.push("ciallo.redb");
        path
    })?;
    let ctx = db.begin_write()?;
    {
        ctx.open_table(models::config::TABLE)?;
        ctx.open_table(models::user::TABLE)?;
        ctx.open_table(models::media::TABLE)?;
    }
    ctx.commit()?;

    DB_CONN
        .set(db)
        .map_err(|_| anyhow!("Failed to set database connection"))?;

    // If the user table is empty, create a default user
    if {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(models::user::TABLE)?;
        table.len()? == 0
    } {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_write()?;
        {
            let mut table = ctx.open_table(models::user::TABLE)?;

            let profile = models::user::Model {
                updated_at: Utc::now(),
                password_hash: generate_hash("admin")?,
                permission: Permission::Root,
                email: "admin@localhost".to_string(),
            };
            let raw = postcard::to_allocvec(&profile)?;
            table.insert("admin", &raw.as_slice())?;
        }
        ctx.commit()?;
    }

    info!("Database is ready");

    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub async fn init() -> Result<()> {
    Ok(())
}
