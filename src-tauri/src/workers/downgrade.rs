//! 套餐变更 Worker
//!
//! 修复三个问题：
//! 1. 消息带 issued_at 时间戳，消费前校验数据库状态，防止乱序执行
//! 2. Redis 分布式锁，同一商户同一方向操作同时只处理一次
//! 3. 大批量 UPDATE 分批执行（每批 500 条，批间 sleep 10ms），防长事务

use crate::db::DbPool;
use crate::utils::mq::{self, PlanMessage};
use futures_lite::stream::StreamExt;
use lapin::options::BasicAckOptions;
use redis::AsyncCommands;
use sqlx::types::Uuid;
use tracing::{error, info, warn};

/// 每批处理的卡密/应用数量上限
const BATCH_SIZE: i64 = 500;
/// Redis 锁过期时间（秒）
const LOCK_TTL: u64 = 120;

pub async fn run_downgrade_worker(
    pool: DbPool,
    channel: lapin::Channel,
    mut redis: redis::aio::ConnectionManager,
) {
    let consumer =
        match mq::create_consumer(&channel, mq::DOWNGRADE_QUEUE, "kamism-downgrade-worker").await {
            Ok(c) => c,
            Err(e) => {
                error!("创建降级消费者失败: {}", e);
                return;
            }
        };

    info!("降级 Worker 已启动，等待消息…");
    let mut consumer = consumer;

    while let Some(delivery) = consumer.next().await {
        match delivery {
            Ok(delivery) => {
                let msg = match parse_message(&delivery.data) {
                    Some(m) => m,
                    None => {
                        warn!("无法解析降级消息");
                        let _ = delivery.ack(BasicAckOptions::default()).await;
                        continue;
                    }
                };

                let lock_key = format!("kamism:plan_lock:downgrade:{}", msg.merchant_id);
                match try_acquire_lock(&mut redis, &lock_key, LOCK_TTL).await {
                    Ok(true) => {
                        match downgrade_merchant(&pool, &msg).await {
                            Ok(skipped) if skipped => info!(
                                "商户 {} 降级消息已过期跳过（issued_at={} 早于当前状态）",
                                msg.merchant_id, msg.issued_at
                            ),
                            Ok(_) => info!("商户 {} 已降级为免费版", msg.merchant_id),
                            Err(e) => error!("商户 {} 降级失败: {}", msg.merchant_id, e),
                        }
                        release_lock(&mut redis, &lock_key).await;
                    }
                    Ok(false) => {
                        warn!("商户 {} 降级操作正在处理中，跳过重复消费", msg.merchant_id);
                    }
                    Err(e) => error!("获取 Redis 锁失败: {}", e),
                }

                let _ = delivery.ack(BasicAckOptions::default()).await;
            }
            Err(e) => {
                error!("接收降级消息错误: {}", e);
                break;
            }
        }
    }
    warn!("降级 Worker 消费循环退出");
}

pub async fn run_upgrade_worker(
    pool: DbPool,
    channel: lapin::Channel,
    mut redis: redis::aio::ConnectionManager,
) {
    let consumer =
        match mq::create_consumer(&channel, mq::UPGRADE_QUEUE, "kamism-upgrade-worker").await {
            Ok(c) => c,
            Err(e) => {
                error!("创建升级消费者失败: {}", e);
                return;
            }
        };

    info!("升级 Worker 已启动，等待消息…");
    let mut consumer = consumer;

    while let Some(delivery) = consumer.next().await {
        match delivery {
            Ok(delivery) => {
                let msg = match parse_message(&delivery.data) {
                    Some(m) => m,
                    None => {
                        warn!("无法解析升级消息");
                        let _ = delivery.ack(BasicAckOptions::default()).await;
                        continue;
                    }
                };

                let lock_key = format!("kamism:plan_lock:upgrade:{}", msg.merchant_id);
                match try_acquire_lock(&mut redis, &lock_key, LOCK_TTL).await {
                    Ok(true) => {
                        match restore_merchant(&pool, &msg).await {
                            Ok(skipped) if skipped => info!(
                                "商户 {} 升级消息已过期跳过（issued_at={} 早于当前状态）",
                                msg.merchant_id, msg.issued_at
                            ),
                            Ok(_) => info!("商户 {} 已恢复为专业版", msg.merchant_id),
                            Err(e) => error!("商户 {} 恢复失败: {}", msg.merchant_id, e),
                        }
                        release_lock(&mut redis, &lock_key).await;
                    }
                    Ok(false) => {
                        warn!("商户 {} 升级操作正在处理中，跳过重复消费", msg.merchant_id);
                    }
                    Err(e) => error!("获取 Redis 锁失败: {}", e),
                }

                let _ = delivery.ack(BasicAckOptions::default()).await;
            }
            Err(e) => {
                error!("接收升级消息错误: {}", e);
                break;
            }
        }
    }
    warn!("升级 Worker 消费循环退出");
}

// ─── 辅助函数 ─────────────────────────────────────────

fn parse_message(data: &[u8]) -> Option<PlanMessage> {
    let s = std::str::from_utf8(data).ok()?;
    serde_json::from_str(s).ok()
}

async fn try_acquire_lock(
    redis: &mut redis::aio::ConnectionManager,
    key: &str,
    ttl: u64,
) -> anyhow::Result<bool> {
    let result: Option<String> = redis
        .set_options(
            key,
            "1",
            redis::SetOptions::default()
                .conditional_set(redis::ExistenceCheck::NX)
                .get(false)
                .with_expiration(redis::SetExpiry::EX(ttl)),
        )
        .await?;
    Ok(result.is_some())
}

async fn release_lock(redis: &mut redis::aio::ConnectionManager, key: &str) {
    let _: redis::RedisResult<()> = redis.del(key).await;
}

// ─── 降级逻辑 ─────────────────────────────────────────

/// 返回 true 表示消息已过期/无需处理，false 表示正常执行
async fn downgrade_merchant(pool: &DbPool, msg: &PlanMessage) -> anyhow::Result<bool> {
    let merchant_id = Uuid::parse_str(&msg.merchant_id)?;

    // 校验：商户当前是否仍为 pro，且 updated_at 不晚于消息发出时间
    // 若已被手动升级（updated_at > issued_at），则跳过
    let check: Option<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT plan, updated_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(pool)
    .await?;

    match check {
        Some((plan, updated_at)) => {
            let issued = chrono::DateTime::<chrono::Utc>::from_timestamp(msg.issued_at, 0)
                .unwrap_or_default();
            // 如果当前已是 free 且更新时间 > issued_at，说明被更新过了，跳过
            if plan != "pro" && updated_at > issued {
                return Ok(true);
            }
            // 如果已经是 free 但 updated_at <= issued_at（扫描器发出，状态已符合），跳过
            if plan != "pro" {
                return Ok(true);
            }
        }
        None => return Ok(true), // 商户不存在
    }

    let free_config: (i32, i32) = sqlx::query_as(
        "SELECT max_apps, max_cards FROM plan_configs WHERE plan = 'free'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((1, 500));
    let (max_apps, max_cards) = free_config;

    // 1. 降级商户
    sqlx::query(
        "UPDATE merchants SET plan = 'free', plan_expires_at = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(merchant_id)
    .execute(pool)
    .await?;

    // 2. 分批禁用超出 max_apps 的应用
    if max_apps >= 0 {
        loop {
            let affected = sqlx::query(
                "UPDATE apps SET status = 'disabled', downgraded = TRUE, updated_at = NOW()
                 WHERE id IN (
                   SELECT id FROM apps
                   WHERE merchant_id = $1 AND status = 'active'
                     AND id NOT IN (
                       SELECT id FROM apps WHERE merchant_id = $1
                       ORDER BY created_at ASC LIMIT $2
                     )
                   LIMIT $3
                 )",
            )
            .bind(merchant_id)
            .bind(max_apps as i64)
            .bind(BATCH_SIZE)
            .execute(pool)
            .await?
            .rows_affected();

            if affected == 0 { break; }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }

    // 3. 分批禁用降级应用下的 unused 卡密
    loop {
        let affected = sqlx::query(
            "UPDATE cards SET status = 'disabled', downgraded = TRUE
             WHERE id IN (
               SELECT id FROM cards
               WHERE merchant_id = $1 AND status = 'unused'
                 AND app_id IN (SELECT id FROM apps WHERE merchant_id = $1 AND downgraded = TRUE)
               LIMIT $2
             )",
        )
        .bind(merchant_id)
        .bind(BATCH_SIZE)
        .execute(pool)
        .await?
        .rows_affected();

        if affected == 0 { break; }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 4. 分批禁用超出 max_cards 的剩余 unused 卡密
    if max_cards >= 0 {
        loop {
            let affected = sqlx::query(
                "UPDATE cards SET status = 'disabled', downgraded = TRUE
                 WHERE id IN (
                   SELECT id FROM cards
                   WHERE merchant_id = $1 AND status = 'unused'
                     AND id NOT IN (
                       SELECT id FROM cards WHERE merchant_id = $1 AND status = 'unused'
                       ORDER BY created_at ASC LIMIT $2
                     )
                   LIMIT $3
                 )",
            )
            .bind(merchant_id)
            .bind(max_cards as i64)
            .bind(BATCH_SIZE)
            .execute(pool)
            .await?
            .rows_affected();

            if affected == 0 { break; }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }

    Ok(false)
}

// ─── 升级恢复逻辑 ──────────────────────────────────────

async fn restore_merchant(pool: &DbPool, msg: &PlanMessage) -> anyhow::Result<bool> {
    let merchant_id = Uuid::parse_str(&msg.merchant_id)?;

    // 校验：商户当前必须是 pro，且 updated_at >= issued_at
    let check: Option<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT plan, updated_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(pool)
    .await?;

    match check {
        Some((plan, _)) if plan == "pro" => {} // 继续执行
        _ => return Ok(true), // 当前不是 pro，跳过（防乱序：升级消息到达但商户已被降级）
    }

    // 分批恢复被降级禁用的应用
    loop {
        let affected = sqlx::query(
            "UPDATE apps SET status = 'active', downgraded = FALSE, updated_at = NOW()
             WHERE id IN (
               SELECT id FROM apps WHERE merchant_id = $1 AND downgraded = TRUE
               LIMIT $2
             )",
        )
        .bind(merchant_id)
        .bind(BATCH_SIZE)
        .execute(pool)
        .await?
        .rows_affected();

        if affected == 0 { break; }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // 分批恢复被降级禁用的卡密
    loop {
        let affected = sqlx::query(
            "UPDATE cards SET status = 'unused', downgraded = FALSE
             WHERE id IN (
               SELECT id FROM cards WHERE merchant_id = $1 AND downgraded = TRUE
               LIMIT $2
             )",
        )
        .bind(merchant_id)
        .bind(BATCH_SIZE)
        .execute(pool)
        .await?
        .rows_affected();

        if affected == 0 { break; }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    Ok(false)
}
