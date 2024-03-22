use anyhow::Result;

use crate::functions::backend::media_insert_log::list;

pub async fn list_file(offset: usize, limit: usize) -> Result<Vec<String>> {
    let logs = list(offset, limit).await?;
    let ret = logs.into_iter().map(|log| log.hash).collect();

    Ok(ret)
}
