use anyhow::Result;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use tairitsu_database::prelude::*;

#[derive(Clone)]
pub struct RouteEnv {
    pub sql: Arc<Box<DatabaseConnection>>,
    pub kv: RouteEnvKV,
    pub bucket: RouteEnvBucket,
}

#[derive(Clone)]
pub struct RouteEnvKV {
    pub token_expired: Arc<Box<ProxyKV>>,
    pub global_config: Arc<Box<ProxyKV>>,
    pub images: Arc<Box<ProxyKV>>,
}

#[derive(Clone)]
pub struct RouteEnvBucket {
    pub static_resources: Arc<Box<ProxyBucket>>,
    pub images: Arc<Box<ProxyBucket>>,
}

#[derive(Clone)]
pub enum InitRouteEnvParams {
    Cloudflare(worker::Env),
    Native,
    WASI,
}

cfg_if::cfg_if! {
    if #[cfg(feature = "cloudflare")]{
        impl RouteEnv {
            pub async fn new(param: InitRouteEnvParams) -> Result<Self> {
                match param {
                    InitRouteEnvParams::Cloudflare(env) => {
                        Ok(Self {
                            sql: Arc::new(
                                init_db(InitSQLParams::Cloudflare {
                                    env: Arc::new(env.clone()),
                                    name: "site".to_string(),
                                })
                                .await?,
                            ),
                            kv: RouteEnvKV {
                                token_expired: Arc::new(
                                    init_kv(InitKVParams::Cloudflare {
                                        env: Arc::new(env.clone()),
                                        name: "token-expired".to_string(),
                                    })
                                    .await?,
                                ),
                                global_config: Arc::new(
                                    init_kv(InitKVParams::Cloudflare {
                                        env: Arc::new(env.clone()),
                                        name: "global-config".to_string(),
                                    })
                                    .await?,
                                ),
                                images: Arc::new(
                                    init_kv(InitKVParams::Cloudflare {
                                        env: Arc::new(env.clone()),
                                        name: "origin-videos".to_string(),
                                    })
                                    .await?,
                                ),
                            },
                            bucket: RouteEnvBucket {
                                static_resources: Arc::new(
                                    init_bucket(InitBucketParams::Cloudflare {
                                        env: Arc::new(env.clone()),
                                        bucket_name: "static".to_string(),
                                        multipart_kv_name: "global-config".to_string(),
                                    })
                                    .await?,
                                ),
                                images: Arc::new(
                                    init_bucket(InitBucketParams::Cloudflare {
                                        env: Arc::new(env.clone()),
                                        bucket_name: "origin-videos".to_string(),
                                        multipart_kv_name: "global-config".to_string(),
                                    })
                                    .await?,
                                ),
                            },
                        })
                    }
                    _ => unreachable!("Only allow one platform at a time"),
                }
            }
        }
    } else if #[cfg(feature = "native")]{
        impl RouteEnv {
            pub async fn new(param: InitRouteEnvParams) -> Result<Self> {
                if cfg!(debug_assertions) {
                    let mut pwd = std::env::current_dir().map_err(
                        |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                    )?;
                    pwd.push("target/cache");
                    std::fs::create_dir_all(&pwd).map_err(
                        |err| anyhow::anyhow!("Failed to create directory: {}", err)
                    )?;
                } else {
                    std::fs::create_dir_all("/home/cache/sql").map_err(
                        |err| anyhow::anyhow!("Failed to create directory: {}", err)
                    )?;
                    std::fs::create_dir_all("/home/cache/kv").map_err(
                        |err| anyhow::anyhow!("Failed to create directory: {}", err)
                    )?;
                    std::fs::create_dir_all("/home/cache/bucket").map_err(
                        |err| anyhow::anyhow!("Failed to create directory: {}", err)
                    )?;
                }

                match param {
                    InitRouteEnvParams::Native => {
                        Ok(Self {
                            sql: Arc::new(init_db(InitSQLParams::Native {
                                url: {
                                    #[cfg(debug_assertions)]
                                    {
                                        let mut pwd = std::env::current_dir().map_err(
                                            |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                                        )?;
                                        pwd.push("target/cache/site.db");
                                        format!("sqlite://{}?mode=rwc", pwd.to_string_lossy())
                                    }
                                    #[cfg(not(debug_assertions))]
                                    {
                                        "sqlite:/home/cache/sql/site.db?mode=rwc".to_string()
                                    }
                                }
                            }).await?),
                            kv: RouteEnvKV {
                                token_expired: Arc::new(
                                    init_kv(InitKVParams::Native {
                                        path: {
                                            #[cfg(debug_assertions)]
                                            {
                                                let mut pwd = std::env::current_dir().map_err(
                                                    |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                                                )?;
                                                pwd.push("target/cache/token-expired.db");
                                                pwd.to_string_lossy().to_string()
                                            }
                                            #[cfg(not(debug_assertions))]
                                            {
                                                "/home/cache/kv/token-expired.db".to_string()
                                            }
                                        }
                                    })
                                    .await?,
                                ),
                                global_config: Arc::new(
                                    init_kv(InitKVParams::Native {
                                        path: {
                                            #[cfg(debug_assertions)]
                                            {
                                                let mut pwd = std::env::current_dir().map_err(
                                                    |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                                                )?;
                                                pwd.push("target/cache/global-config.db");
                                                pwd.to_string_lossy().to_string()
                                            }
                                            #[cfg(not(debug_assertions))]
                                            {
                                                "/home/cache/kv/global-config.db".to_string()
                                            }
                                        }
                                    })
                                    .await?,
                                ),
                                images: Arc::new(
                                    init_kv(InitKVParams::Native {
                                        path: {
                                            #[cfg(debug_assertions)]
                                            {
                                                let mut pwd = std::env::current_dir().map_err(
                                                    |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                                                )?;
                                                pwd.push("target/cache/images.db");
                                                pwd.to_string_lossy().to_string()
                                            }
                                            #[cfg(not(debug_assertions))]
                                            {
                                                "/home/cache/kv/images.db".to_string()
                                            }
                                        }
                                    })
                                    .await?,
                                ),
                            },
                            bucket: RouteEnvBucket {
                                static_resources: Arc::new(
                                    init_bucket(InitBucketParams::Native {
                                        path: {
                                            #[cfg(debug_assertions)]
                                            {
                                                let mut pwd = std::env::current_dir().map_err(
                                                    |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                                                )?;
                                                pwd.push("target/cache/static-resources");

                                                std::fs::create_dir_all(&pwd).map_err(
                                                    |err| anyhow::anyhow!("Failed to create directory: {}", err)
                                                )?;

                                                pwd.to_string_lossy().to_string()
                                            }
                                            #[cfg(not(debug_assertions))]
                                            {
                                                "/home/cache/bucket/static-resources".to_string()
                                            }
                                        }
                                    })
                                    .await?,
                                ),
                                images: Arc::new(
                                    init_bucket(InitBucketParams::Native {
                                        path: {
                                            #[cfg(debug_assertions)]
                                            {
                                                let mut pwd = std::env::current_dir().map_err(
                                                    |err| anyhow::anyhow!("Failed to get current directory: {}", err)
                                                )?;
                                                pwd.push("target/cache/images");

                                                std::fs::create_dir_all(&pwd).map_err(
                                                    |err| anyhow::anyhow!("Failed to create directory: {}", err)
                                                )?;

                                                pwd.to_string_lossy().to_string()
                                            }
                                            #[cfg(not(debug_assertions))]
                                            {
                                                "/home/cache/bucket/images".to_string()
                                            }
                                        }
                                    })
                                    .await?,
                                ),
                            },
                        })
                    }
                    _ => unreachable!("Only allow one platform at a time"),
                }
            }
        }
    } else if #[cfg(feature = "wasi")] {
        impl RouteEnv {
            pub async fn new(param: InitRouteEnvParams) -> Result<Self> {
                match param {
                    InitRouteEnvParams::WASI => {
                        Ok(Self {
                            sql: Arc::new(init_db(InitSQLParams::WASI).await?),
                            kv: RouteEnvKV {
                                token_expired: Arc::new(
                                    init_kv(InitKVParams::WASI {
                                        name: "token-expired".to_string()
                                    }).await?,
                                ),
                                global_config: Arc::new(
                                    init_kv(InitKVParams::WASI {
                                        name: "global-config".to_string()
                                    }).await?,
                                ),
                                images: Arc::new(
                                    init_kv(InitKVParams::WASI {
                                        name: "images".to_string(),
                                    })
                                    .await?,
                                ),
                            },
                            bucket: RouteEnvBucket {
                                static_resources: Arc::new(
                                    init_bucket(InitBucketParams::WASI {
                                        name: "static".to_string()
                                    }).await?,
                                ),
                                images: Arc::new(
                                    init_bucket(InitBucketParams::WASI {
                                        name: "origin-videos".to_string()
                                    }).await?,
                                ),
                            },
                        })
                    }
                    _ => unreachable!("Only allow one platform at a time"),
                }
            }
        }
    }
}
