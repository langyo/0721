use anyhow::{anyhow, Result};

use sea_orm::{
    ColumnTrait, EntityTrait, NotSet, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
    Unchanged,
};

use crate::{functions::frontend::auth::generate_hash, init::RouteEnv};
use _types::{
    models::user::*,
    request::{Permission, RegisterParams},
    response::UserBasicInfo,
};
use tairitsu_database::prelude::*;

pub async fn get(env: RouteEnv, email: String) -> Result<Option<Model>> {
    let ret = Entity::find()
        .filter(Column::Email.eq(email))
        .one(&**env.sql)
        .await?;

    Ok(ret)
}

pub async fn count(env: RouteEnv) -> Result<u64> {
    let count = Entity::find().count(&**env.sql).await?;

    Ok(count)
}

pub async fn list(env: RouteEnv, offset: usize, limit: usize) -> Result<Vec<UserBasicInfo>> {
    let ret = Entity::find()
        .offset(offset as u64)
        .limit(limit as u64)
        .order_by_asc(Column::Id)
        .all(&**env.sql)
        .await?;
    let ret = ret
        .iter()
        .map(|item| item.to_owned().try_into())
        .collect::<Vec<Result<UserBasicInfo>>>()
        .into_iter()
        .collect::<Result<Vec<UserBasicInfo>>>()?;

    Ok(ret)
}

pub async fn set(env: RouteEnv, params: RegisterParams) -> Result<()> {
    if let Some(ret) = Entity::find()
        .filter(Column::Name.eq(params.name.clone()))
        .one(&**env.sql)
        .await?
    {
        let ret = ActiveModel {
            id: Unchanged(ret.id),
            name: NotSet,
            permission: if let Some(permission) = params.permission {
                Set(serde_json::to_string(&permission)?)
            } else {
                NotSet
            },
            password_hash: if let Some(password_raw) = params.password_raw {
                Set(generate_hash(&password_raw)?)
            } else {
                NotSet
            },
            email: NotSet,
            extra_profile: NotSet,
        };

        Entity::insert(ret).exec(&**env.sql).await?;
    } else {
        let user = ActiveModel {
            id: NotSet,
            name: Set(params.name.clone().ok_or(anyhow!(
                "Name is required to create a new user, but it was not provided"
            ))?),
            permission: if let Some(permission) = params.permission {
                Set(serde_json::to_string(&permission)?)
            } else {
                Set(serde_json::to_string(&Permission::User)?)
            },
            password_hash: if let Some(password_raw) = params.password_raw {
                Set(generate_hash(&password_raw)?)
            } else {
                NotSet
            },
            email: Set(params.email.clone()),
            extra_profile: NotSet,
        };

        Entity::insert(user).exec(&**env.sql).await?;
    }

    env.kv
        .token_expired
        .set(params.email, chrono::Utc::now().to_rfc3339())
        .await?;
    Ok(())
}

pub async fn delete(env: RouteEnv, email: String) -> Result<()> {
    Entity::delete(ActiveModel {
        email: Set(email),
        ..Default::default()
    })
    .exec(&**env.sql)
    .await?;

    Ok(())
}
