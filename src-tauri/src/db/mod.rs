use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

pub mod encrypted_fields;

pub type DbPool = PgPool;

pub async fn create_pool(database_url: &str) -> Result<DbPool> {
    let pool = PgPoolOptions::new()
        // 最大连接数：根据 CPU 核心数 * 2 + 有效磁盘数（一般为 CPU*2+1）
        .max_connections(20)
        // 保持热连接，避免冷启动延迟
        .min_connections(2)
        // 等待连接超时：10s
        .acquire_timeout(Duration::from_secs(10))
        // 空闲连接最长存活时间：30min（防止数据库端超时关闭）
        .idle_timeout(Duration::from_secs(1800))
        // 连接最长生命周期：1h（避免使用到半关闭连接）
        .max_lifetime(Duration::from_secs(3600))
        // 获取连接前测试是否存活（健康检查），避免使用过期连接
        .test_before_acquire(true)
        .connect(database_url)
        .await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await?;
    Ok(())
}
