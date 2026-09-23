//! 套餐变更 Worker
//!
//! 修复三个问题：
//! 1. 消息带 issued_at 时间戳，消费前校验数据库状态，防止乱序执行
//! 2. Redis 分布式锁，同一商户同一方向操作同时只处理一次
//! 3. 大批量 UPDATE 分批执行（每批 500 条，批间 sleep 10ms），防长事务

use crate::db::DbPool;
use crate::utils::mq::{self, PlanMessage};
use crate::utils::redis_guard;
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
                                "商户 {} 的 {} 消息已过期跳过（issued_at={} 早于当前状态）",
                                msg.merchant_id, msg.action, msg.issued_at
                            ),
                            // action 决定这条消息做了什么：
                            //   downgrade → 清理 + 改 plan；cleanup → 只清理（plan 已由调用方改好）
                            Ok(_) => info!("商户 {} 的 {} 处理完成", msg.merchant_id, msg.action),
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
    // 释放失败的方向是**安全**的：锁会留到 `LOCK_TTL`(120s) 到期，期间该商户的同类
    // 消息会被判成「正在处理中，跳过重复消费」—— 即「多等一会儿」而不是「重复执行」。
    // 而到期扫描每 60s 会重新投递（plan 尚未改成 free 的商户仍在筛选集里），
    // 所以最多延迟一个周期就会自愈。属派生态，best_effort。
    redis_guard::best_effort::<()>(redis.del(key).await, "释放降级/升级分布式锁");
}

// ─── 降级逻辑 ─────────────────────────────────────────

/// 返回 true 表示消息已过期/无需处理，false 表示正常执行
async fn downgrade_merchant(pool: &DbPool, msg: &PlanMessage) -> anyhow::Result<bool> {
    let merchant_id = Uuid::parse_str(&msg.merchant_id)?;

    // 「仅清理」模式：调用方（管理员手动降级）已经把 `plan` 改成 free，
    // 这条消息只负责清理超额资源，不改 plan。
    let cleanup_only = msg.action == "cleanup";

    // 消息发出时间（秒级）。完整降级路径用它做两件事：乱序校验、以及末尾那条 UPDATE 的守卫。
    // 提到函数级解析是刻意的 —— 校验和写入必须共用同一个时间基准，
    // 否则「预检通过、写入时又不认」这类错位会很难查。
    //
    // ⚠️ 仅清理模式**不用**它：管理员路径刚刚把 `merchants.updated_at` 设成 NOW()，
    // 而这里是秒级截断 —— `updated_at > issued_at` 会恒成立，消息次次被跳过（实测踩到过）。
    let issued = match chrono::DateTime::<chrono::Utc>::from_timestamp(msg.issued_at, 0) {
        Some(t) => t,
        None => {
            warn!("商户 {} 降级消息 issued_at 无效: {}", msg.merchant_id, msg.issued_at);
            return Ok(true);
        }
    };

    // 预检：判断这条消息还该不该执行。
    // ⚠️ 这一步只是「快速失败」，避免为一份注定要跳过的消息白做清理工作 ——
    // **它不构成并发保护**。真正的保护是末尾那条带守卫的 UPDATE（见下），
    // 因为预检与实际写入之间隔着一次配额查询，是两次独立的数据库往返。
    let check: Option<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT plan, updated_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(pool)
    .await?;

    match check {
        Some((plan, updated_at)) => {
            if cleanup_only {
                // plan 已由调用方改成 free。若现在又是 pro，说明商户被重新升级了，
                // 这条陈旧消息不该再去禁他的资源。
                if plan == "pro" {
                    info!("商户 {} 已被重新升级为 pro，跳过清理", msg.merchant_id);
                    return Ok(true);
                }
            } else {
                // 如果商户状态在消息发出后被修改过（如续费），跳过此消息
                if updated_at > issued {
                    info!("商户 {} 状态在降级消息发出后已变更，跳过", msg.merchant_id);
                    return Ok(true);
                }
                // 如果已经是 free，跳过
                if plan != "pro" {
                    return Ok(true);
                }
            }
        }
        None => return Ok(true), // 商户不存在
    }

    // ⚠️ 曾用「查询失败就用默认值 (1, 500)」兜底。方向看似安全（配额更严），但那是错的：
    // 下面几步要**禁用商户的应用和卡密**，属于破坏性操作 ——
    // 拿一个猜来的配额去删用户的东西，比什么都不做更糟。
    // 而且运营可以在「套餐配置」页改 free 的配额，硬编码值必然与真实配置脱节：
    // 比如 free 已被调成 max_apps = 2，这里却按 1 去禁用，就会多禁掉一个应用。
    // 查询失败就整体失败 —— 此时 merchants.plan 还没改（见下面的顺序说明），
    // 商户仍会被下一轮扫描选中，可以安全重试。
    let (max_apps, max_cards): (i32, i32) = sqlx::query_as(
        "SELECT max_apps, max_cards FROM plan_configs WHERE plan = 'free'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| {
        anyhow::anyhow!(
            "查询免费版配额失败，放弃本次降级（不做破坏性禁用，下轮扫描会重试）: {}",
            e
        )
    })?;

    // ── 顺序说明（改这段之前请读完）──────────────────────────────────────
    // 下面三步是「清理」，最后一步才「改 plan」。这个顺序不是随意的：
    // 扫描器用 `plan = 'pro' AND plan_expires_at <= NOW()` 选商户（见 lib.rs::scan_and_enqueue），
    // 只要 plan 还没改，中途任何一步失败都一定会被下一个 tick 重新扫到、重新投递 —— 天然可重试。
    //
    // 反之（先改 plan 再清理）会把商户**移出扫描集合**：清理失败后没有任何机制再回来，
    // 留下「plan = free 但超额 apps / 卡密仍 active」的永久不一致，日志里只有一行 error，
    // 而平台会一直以为这个商户已经降级到位了。
    //
    // 前置保证：下面三步各自幂等（每批 UPDATE 都带 status 守卫，重复执行不会重复伤害）。
    // 幂等性推导见 `.workbuddy/plan-expiry-analysis.md` 附录 B。

    // 1. 分批禁用超出 max_apps 的应用
    if max_apps >= 0 {
        loop {
            let affected = sqlx::query(
                "UPDATE apps SET status = 'disabled', downgraded = TRUE, updated_at = NOW()
                 WHERE id IN (
                   SELECT id FROM apps
                   WHERE merchant_id = $1 AND status = 'active'
                     AND id NOT IN (
                       -- 保留集合也必须限定 status = 'active'：否则商户**自己禁用**的应用
                       -- （status = 'disabled'）会占掉保留名额。极端情况下 max_apps 个名额
                       -- 全被已禁用的应用占住，结果把商户唯一可用的应用也禁掉 ——
                       -- 商户付了钱，出来一个能用的都没有。
                       -- 注意 cards 那侧的同类子查询本来就带 status 守卫，这里是补齐。
                       SELECT id FROM apps WHERE merchant_id = $1 AND status = 'active'
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

    // 2. 分批禁用降级应用下的 unused 卡密
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

    // 3. 分批禁用超出 max_cards 的剩余 unused 卡密
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

    // 4. 改 plan —— 只有「完整降级」才走到这里，这是它的**提交点**
    //
    // 仅清理模式到此为止：plan 已经由管理员接口改成了 free，这里只负责把超额资源禁掉。
    if cleanup_only {
        info!(
            "商户 {} 超额资源清理完成（仅清理模式，不动 plan）",
            msg.merchant_id
        );
        return Ok(false);
    }

    // 守卫 `plan = 'pro' AND updated_at <= $2` 把「校验」和「写入」合并成一个原子操作，
    // 消除了预检与写入之间的 TOCTOU 窗口（两者中间隔着一次 plan_configs 查询）。
    // 若商户在这中间续了费 —— plan 仍是 pro，但 updated_at 变新 —— 这条 UPDATE 命中 0 行。
    let updated = sqlx::query(
        "UPDATE merchants SET plan = 'free', plan_expires_at = NULL, updated_at = NOW()
         WHERE id = $1 AND plan = 'pro' AND updated_at <= $2",
    )
    .bind(merchant_id)
    .bind(issued)
    .execute(pool)
    .await?;

    if updated.rows_affected() == 0 {
        // 走到这里说明：预检之后、提交之前，商户状态被改了（典型场景是刚好续费成功）。
        // 上面的清理**已经执行过**，必须撤销 —— 否则会误伤刚付费的用户：
        // 他付了钱、收到「续费成功」，转头发现应用和卡密被禁用了。
        //
        // 这里直接调恢复函数，而不是补发一条 upgrade 消息：升级消息的乱序校验是
        // `updated_at > issued_at`，而商户的 updated_at 恰好就是刚续费的那一刻 ——
        // 补发的消息会被判为「已过期」跳过，起不到恢复作用。
        warn!(
            "商户 {} 在降级过程中状态已变更（可能刚续费），撤销本次清理",
            msg.merchant_id
        );
        restore_downgraded_resources(pool, merchant_id).await?;
        return Ok(true);
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
        Some((plan, updated_at)) => {
            let issued = match chrono::DateTime::<chrono::Utc>::from_timestamp(msg.issued_at, 0) {
                Some(t) => t,
                None => {
                    warn!("商户 {} 升级消息 issued_at 无效: {}", msg.merchant_id, msg.issued_at);
                    return Ok(true);
                }
            };
            // 如果商户状态在消息发出后被修改过（如被管理员降级），跳过
            if updated_at > issued {
                info!("商户 {} 状态在升级消息发出后已变更，跳过", msg.merchant_id);
                return Ok(true);
            }
            if plan != "pro" {
                return Ok(true); // 当前不是 pro，跳过
            }
        }
        None => return Ok(true), // 商户不存在
    }

    restore_downgraded_resources(pool, merchant_id).await?;

    Ok(false)
}

/// 把因降级而被禁用的 apps / cards 恢复回原状态。
///
/// 两处调用：
/// ① `restore_merchant` —— 收到升级消息，商户重新变成 pro
/// ② `downgrade_merchant` 的收尾 —— 清理做完之后才发现商户状态已变（例如刚好续费成功），
///    此时必须撤销清理，否则会误伤刚付费的用户
///
/// 只动 `downgraded = TRUE` 的记录，所以不会把商户**自己**禁用的应用/卡密错误激活。
async fn restore_downgraded_resources(pool: &DbPool, merchant_id: Uuid) -> anyhow::Result<()> {
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

    Ok(())
}
