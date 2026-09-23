//! 对外公开 API，供第三方软件调用（使用 api_key 鉴权，无需 JWT）

use redis::AsyncCommands;
use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::AppState,
    models::{activation::Activation, card::Card},
    utils::{db_guard, mask, redis_guard},
};
use axum::{
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ActivateRequest {
    pub api_key: String,
    pub app_id: Uuid,
    pub card_code: String,
    pub device_id: String,
    pub device_name: Option<String>,
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    pub api_key: String,
    pub app_id: Uuid,
    pub card_code: String,
    pub device_id: String,
}

#[derive(Deserialize)]
pub struct UnbindRequest {
    pub api_key: String,
    pub app_id: Uuid,
    pub card_code: String,
    pub device_id: String,
}

pub fn public_api_router(state: AppState) -> Router<AppState> {
    use crate::middleware::rate_limit::{api_rate_limit, activate_rate_limit};
    use axum::middleware;
    Router::new()
        // /v1/activate 叠加激活专用限流（更严格，20次/分钟）
        .route("/v1/activate",
            post(activate).route_layer(
                middleware::from_fn_with_state(state.clone(), activate_rate_limit)
            )
        )
        .route("/v1/verify", post(verify))
        .route("/v1/unbind", post(unbind))
        .route_layer(middleware::from_fn_with_state(state, api_rate_limit))
}

/// 激活卡密
async fn activate(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ActivateRequest>,
) -> Json<Value> {
    if body.device_id.trim().is_empty() {
        return Json(json!({"success": false, "message": "设备ID不能为空"}));
    }

    // 查询所有商户并使用哈希索引查询 API Key
    //
    // ⚠️ 这里曾用 `.unwrap_or(None)` —— 后果是**数据库一抖，所有商户同时收到
    // 「无效的 API Key」**，而日志上一个字都没有。排查的人会去查密钥，
    // 真正的问题却在数据库。现在把「查不到」和「查不了」分开。
    let api_key_hash = EncryptedFieldsOps::generate_hash(&body.api_key);
    let merchant = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'",
        )
        .bind(&api_key_hash)
        .fetch_optional(&state.pool),
        "按 API Key 查询商户",
    )
    .await
    {
        db_guard::QueryOutcome::Found(m) => m,
        // 查通了但没有这个 key —— 正常的业务结论
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "无效的 API Key"}))
        }
        // 查询失败 —— 是故障，不能伪装成「key 无效」
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let merchant_id = merchant.0;

    // 验证 app_id 归属于该商户且处于 active 状态
    match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'",
        )
        .bind(body.app_id)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "校验应用归属",
    )
    .await
    {
        db_guard::QueryOutcome::Found(_) => {}
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "应用不存在或已禁用"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    }

    // 真实客户端 IP：对端是可信代理时才看转发头（见 utils::client_ip 的注释）
    // 同时保留结构化的 IpAddr：限流桶要按 bucket_key 聚合（IPv6 收敛到 /64），
    // 落库和展示则用字符串形式
    let client_ip = state.trusted_proxies.resolve(&headers, addr);
    let ip = client_ip.to_string();

    // ── IP 黑名单检查（全局 + 商户级）──────────────────────────────────────
    //
    // ⚠️ 这里曾用 `.unwrap_or(None)`，方向比「API Key 无效」更危险：
    // 查询失败 → None → 判定为「没被拉黑」→ **风控静默放行**。
    // 也就是说数据库一抖，所有黑名单都不生效了，而且没有任何迹象。
    // 对风控而言，宁可暂时拒绝服务（fail-closed），也不能静默放行。
    match db_guard::optional(
        sqlx::query_as::<_, (i64,)>(
            "SELECT 1 FROM ip_blacklist
             WHERE ip = $1 AND (merchant_id IS NULL OR merchant_id = $2)
             LIMIT 1",
        )
        .bind(&ip)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "查询 IP 黑名单",
    )
    .await
    {
        db_guard::QueryOutcome::Found(_) => {
            return Json(json!({"success": false, "message": "当前 IP 已被限制激活"}))
        }
        db_guard::QueryOutcome::NotFound => {} // 不在黑名单里，正常放行
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    }

    // ── 设备黑名单检查（全局 + 商户级）────────────────────────────────────
    let device_id_hash = EncryptedFieldsOps::generate_hash(&body.device_id);
    match db_guard::optional(
        sqlx::query_as::<_, (i64,)>(
            "SELECT 1 FROM device_blacklist
             WHERE device_id_hash = $1 AND (merchant_id IS NULL OR merchant_id = $2)
             LIMIT 1",
        )
        .bind(&device_id_hash)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "查询设备黑名单",
    )
    .await
    {
        db_guard::QueryOutcome::Found(_) => {
            return Json(json!({"success": false, "message": "当前设备已被限制激活"}))
        }
        db_guard::QueryOutcome::NotFound => {}
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    }

    // ── 异常检测：同一设备激活多张卡 ───────────────────────────────────────
    // 同一 device_id_hash 在该商户下绑定了超过 3 张不同卡密，视为异常
    //
    // ⚠️ 这里曾用 `.unwrap_or((0,))` —— 查询失败时把「设备已绑卡数」当成 0，
    // 于是 `0 >= 3` 为假 → **异常检测自动放行**。数据库一抖，风控就不工作了，
    // 而且和黑名单那两处一样：日志里什么都没有。
    let device_card_count = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(DISTINCT a.card_id) FROM activations a
             JOIN cards c ON c.id = a.card_id
             WHERE a.device_id_hash = $1 AND c.merchant_id = $2",
        )
        .bind(&device_id_hash)
        .bind(merchant_id)
        .fetch_one(&state.pool),
        "统计设备已绑卡数（异常检测）",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(c) => c,
        // 无法判定是否异常时**不放行**：异常检测的作用是拦住可疑行为，
        // 在判定不了的时候放过等于关掉风控。对激活接口来说，
        // 宁可让用户重试一次（几秒后数据库恢复即可），也不要静默放行。
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    if device_card_count.0 >= 3 {
        // ⚠️ 与 blacklist.rs 同一处缺陷：原先 `&body.device_id[..4]` 是**字节**切片，
        // 设备号含多字节字符（中文/emoji）时会 panic。这是 `/v1/activate` 的路径，
        // 设备号由激活方提供，属于外部输入。
        let device_hint = mask::hint(&body.device_id, 4);
        let pool_alert = state.pool.clone();
        let mid = merchant_id;
        let hint = device_hint.clone();
        let ip_clone = ip.clone();
        tokio::spawn(async move {
            // 告警写不进去必须留痕：这条 INSERT 是「同一设备激活了多张卡密」
            // 这个事实**唯一**的记录。写失败而没人知道，等于风控在哑火状态运行。
            // （不阻塞激活是有意的：激活是主流程，告警是旁路。）
            if let Err(e) = sqlx::query(
                "INSERT INTO activation_alerts
                 (merchant_id, alert_type, device_hint, ip_address, detail)
                 VALUES ($1, 'device_multi_card', $2, $3, $4)"
            )
            .bind(mid)
            .bind(&hint)
            .bind(&ip_clone)
            .bind(format!("设备 {} 已激活 {} 张卡密", hint, device_card_count.0 + 1))
            .execute(&pool_alert)
            .await
            {
                tracing::warn!(
                    "写「设备多卡」告警失败，这条异常激活没有留痕: merchant_id={} device_hint={} err={}",
                    mid,
                    hint,
                    e
                );
            }
        });
    }

    // ── 异常检测：同 IP 短时间大量激活（超过阈值写告警）──────────────────
    // Redis 限流已拦截超频请求，此处记录接近阈值的行为（15次/分钟）
    {
        let mut redis = state.redis.clone();
        // 必须用与限流中间件相同的 key（含 IPv6 /64 聚合），否则这里永远读到 0，
        // 异常告警会静默失效
        let rl_key = format!("rl:activate:{}", crate::utils::client_ip::bucket_key(client_ip));
        let count: i64 = redis::AsyncCommands::get(&mut redis, &rl_key)
            .await
            .unwrap_or(0i64);
        if count >= 15 {
            let pool_alert = state.pool.clone();
            let mid = merchant_id;
            let ip_clone = ip.clone();
            tokio::spawn(async move {
                // 同上一处：告警是「同 IP 高频激活」这个事实的唯一记录，失败必须留痕
                if let Err(e) = sqlx::query(
                    "INSERT INTO activation_alerts
                     (merchant_id, alert_type, ip_address, detail)
                     VALUES ($1, 'ip_abuse', $2, $3)
                     ON CONFLICT DO NOTHING"
                )
                .bind(mid)
                .bind(&ip_clone)
                .bind(format!("IP {} 本分钟已激活 {} 次", ip_clone, count))
                .execute(&pool_alert)
                .await
                {
                    tracing::warn!(
                        "写「IP 高频激活」告警失败，这条异常激活没有留痕: merchant_id={} ip={} err={}",
                        mid,
                        ip_clone,
                        e
                    );
                }
            });
        }
    }

    // 事务外先按 (code_hash, merchant_id, app_id) 定位卡密拿 id。
    //
    // 这里只做「定位」，**不做任何状态判断** —— 状态、过期、设备数全部挪进
    // 下面的事务里，在 `cards` 行锁的保护下重读判定。事务外读到的 status/expires_at
    // 在并发下都是过期的快照，拿它做拒绝与否的决策就是「安静地做错事」。
    let code_hash = EncryptedFieldsOps::generate_hash(&body.card_code);
    let card_id: Option<(Uuid,)> = match sqlx::query_as(
        "SELECT id FROM cards WHERE code_hash = $1 AND merchant_id = $2 AND app_id = $3",
    )
    .bind(&code_hash)
    .bind(merchant_id)
    .bind(body.app_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // 不能 `unwrap_or(None)` —— 那会把「数据库抖动」伪装成「卡密不存在」，
            // 客户端拿到的是「卡密不存在」这种看起来正常的业务拒绝，没人会去查日志。
            tracing::error!("查询卡密失败: merchant_id={} app_id={} err={}", merchant_id, body.app_id, e);
            return Json(json!({"success": false, "message": "服务器繁忙，请稍后重试"}));
        }
    };

    let card_id = match card_id {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "卡密不存在"})),
    };

    // ── 事务：锁卡密行 → 状态复核 → 设备去重 → 设备数检查 → 激活 INSERT → 卡状态更新 ──
    //
    // 🔒 为什么第一件事是锁 cards 行，而不是像早期版本那样去锁 activations 行：
    //
    // 早期写法是 `SELECT COUNT(*) FROM (SELECT 1 FROM activations WHERE card_id=$1 FOR UPDATE) AS sub`。
    // 它有两个问题：
    //   1. `FOR UPDATE` 只能锁「读到的行」。当一个设备都还没绑定时，子查询读到 **0 行**，
    //      于是**什么锁都没拿到** —— PG 没有间隙锁（gap lock），后来者同样读到 0 行，
    //      两边都通过 `device_count >= max_devices` 检查。实测：max_devices=3 时
    //      30 并发可以让 9 台不同设备全部激活成功（见 .workbuddy/hypotheses-batch2.md）。
    //   2. `FOR UPDATE` 不能直接用在聚合函数上（PG 报 FeatureNotSupported），
    //      所以这种「数数 + 加锁」的拼接在语法上就只有这一个别扭的写法。
    //
    // 改成锁 cards 行：cards 行**一定存在**（上面刚查出来），是一个稳定、唯一的
    // 串行化点。拿到它之后再数 activations，并发就被真正串起来了。
    let mut tx = match state.pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("开启事务失败: {}", e);
            return Json(json!({"success": false, "message": "服务器繁忙，请稍后重试"}));
        }
    };

    // 事务内重读卡密并加锁。这里同时把状态检查挪进事务 ——
    // 事务外读到的 status 在并发下没有意义（可能刚被后台禁用），
    // 而且下面「已过期就改 status='expired'」的写操作如果走事务外的裸连接，
    // 会和本事务的 `UPDATE cards SET status='active'` 并发写同一行，最后一次写入赢。
    let card: Option<Card> = match sqlx::query_as(
        "SELECT * FROM cards WHERE id = $1 FOR UPDATE",
    )
    .bind(card_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("事务内锁卡密失败: card_id={} err={}", card_id, e);
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "激活失败，请稍后重试"}));
        }
    };

    // 加锁后重读不到 —— 说明卡密在事务开始后被删除/改归属了。
    // 不要静默当成「卡密不存在」以外的任何事，直接按不存在处理。
    let card = match card {
        Some(c) => c,
        None => {
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "卡密不存在"}));
        }
    };

    match card.status.as_str() {
        "disabled" => {
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "卡密已被禁用"}));
        }
        "expired" => {
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "卡密已过期"}));
        }
        _ => {}
    }

    if let Some(exp) = card.expires_at {
        if Utc::now() > exp {
            if let Err(e) = sqlx::query("UPDATE cards SET status = 'expired' WHERE id = $1")
                .bind(card.id)
                .execute(&mut *tx)
                .await
            {
                tracing::error!("标记卡密过期失败: card_id={} err={}", card.id, e);
            }
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "卡密已过期"}));
        }
    }

    // 事务内加锁检查重复（防止并发两个相同 card+device 都通过外层检查）
    let existing_in_tx: Option<(Uuid,)> = match sqlx::query_as(
        "SELECT id FROM activations WHERE card_id = $1 AND device_id_hash = $2 FOR UPDATE",
    )
    .bind(card.id)
    .bind(&device_id_hash)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询重复激活失败: {}", e);
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "激活失败，请稍后重试"}));
        }
    };

    if let Some((existing_id,)) = existing_in_tx {
        if let Err(e) = sqlx::query("UPDATE activations SET last_verified_at = NOW() WHERE id = $1")
            .bind(existing_id)
            .execute(&mut *tx)
            .await
        {
            tracing::error!("更新激活时间失败: {}", e);
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "激活失败"}));
        }
        if let Err(e) = tx.commit().await {
            tracing::error!("提交事务失败: {}", e);
            return Json(json!({"success": false, "message": "激活失败"}));
        }
        let remaining_days = card.expires_at.map(|e| (e - Utc::now()).num_days().max(0) + 1);
        return Json(json!({
            "success": true,
            "message": "卡密已激活（设备已绑定）",
            "data": {
                "expires_at": card.expires_at,
                "remaining_days": remaining_days,
                "max_devices": card.max_devices
            }
        }));
    }

    // 事务内检查设备数量。
    // 这里**不再需要 FOR UPDATE**：串行化由上面 `cards` 行的行锁保证，
    // 同一张卡密的并发激活已经被排队，读到的一定是最新计数。
    //
    // 仍然显式标注 max_devices 语义：它只在**第一次绑定**时受限，
    // 后续重复绑定同一设备走上面的 existing_in_tx 分支，不消耗名额。
    let device_count: (i64,) = match sqlx::query_as(
        "SELECT COUNT(*) FROM activations WHERE card_id = $1",
    )
    .bind(card.id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("查询设备数量失败: {}", e);
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "激活失败，请稍后重试"}));
        }
    };

    if device_count.0 >= card.max_devices as i64 {
        let _ = tx.rollback().await;
        return Json(json!({
            "success": false,
            "message": format!("该卡密最多支持 {} 台设备，已达上限", card.max_devices)
        }));
    }

    let now = Utc::now();
    let expires_at = if card.activated_at.is_none() {
        Some(now + Duration::days(card.duration_days as i64))
    } else {
        card.expires_at
    };

    let activation_id = Uuid::new_v4();

    // 加密设备 ID。device_id_hash 已在上方定义。
    //
    // ⚠️ 这里必须用 `_tx` 变体：本函数内部会往 `encrypted_fields_log` 写一条日志，
    // 如果走 `state.pool` 就是**在事务中途取第二条连接**。两周后果：
    //   - 连接池被自耗（并发时挂起，不报错）
    //   - 事务回滚后日志已落库 → 孤儿日志（实测线上库里已有 14 条）
    // 传 &mut *tx 进去，日志与激活行同生共死。
    let encrypted_device_id = match EncryptedFieldsOps::encrypt_device_id_tx(
        &mut *tx,
        &state.encryptor,
        activation_id,
        &body.device_id,
    ).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("加密设备 ID 失败: {}", e);
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "激活失败"}));
        }
    };

    if let Err(e) = sqlx::query(
        "INSERT INTO activations (id, card_id, app_id, device_id_encrypted, device_id_hash, device_name, ip_address) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(activation_id)
    .bind(card.id)
    .bind(card.app_id)
    .bind(&encrypted_device_id)
    .bind(&device_id_hash)
    .bind(&body.device_name)
    .bind(&ip)
    .execute(&mut *tx)
    .await
    {
        tracing::error!("插入激活记录失败: {}", e);
        let _ = tx.rollback().await;
        return Json(json!({"success": false, "message": "激活失败，请稍后重试"}));
    }

    if let Err(e) = sqlx::query(
        "UPDATE cards SET status = 'active', activated_at = COALESCE(activated_at, NOW()), expires_at = $1 WHERE id = $2",
    )
    .bind(expires_at)
    .bind(card.id)
    .execute(&mut *tx)
    .await
    {
        tracing::error!("更新卡状态失败: {}", e);
        let _ = tx.rollback().await;
        return Json(json!({"success": false, "message": "激活失败，请稍后重试"}));
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("提交事务失败: {}", e);
        return Json(json!({"success": false, "message": "激活失败，请稍后重试"}));
    }

    let remaining_days = expires_at.map(|e| (e - Utc::now()).num_days().max(0) + 1);
    // 异步触发 Webhook（activate 事件）
    let pool_clone = state.pool.clone();
    let app_id_clone = card.app_id;
    let webhook_payload = serde_json::json!({
        "card_code": body.card_code,
        "device_id": body.device_id,
        "device_name": body.device_name,
        "expires_at": expires_at,
        "remaining_days": remaining_days,
    });
    tokio::spawn(async move {
        crate::routes::webhooks::fire_webhook(&pool_clone, app_id_clone, "activate", webhook_payload).await;
    });

    // 异步写分润记录（代理体系）
    let pool_commission = state.pool.clone();
    let mid_commission = merchant_id;
    let card_id_commission = card.id;
    tokio::spawn(async move {
        crate::routes::agent::record_commission(
            &pool_commission,
            mid_commission,
            card_id_commission,
            activation_id,
        ).await;
    });

    Json(json!({
        "success": true,
        "message": "激活成功",
        "data": {
            "expires_at": expires_at,
            "remaining_days": remaining_days,
            "max_devices": card.max_devices,
            "current_devices": device_count.0 + 1
        }
    }))
}

// ────────────────────────────── verify 缓存 ──────────────────────────────

/// `/v1/verify` 的 Redis 缓存 key。
///
/// ⚠️ 任何想主动失效这个缓存的地方都必须调用本函数来构造 key。
/// 不要手写格式 —— 格式一旦分叉，删缓存会静默地删不到，
/// 而「删不到的缓存」不会报错，只会安静地返回过期结果。
fn verify_cache_key(api_key: &str, app_id: Uuid, card_code: &str, device_id: &str) -> String {
    let api_key_hash   = EncryptedFieldsOps::generate_hash(api_key);
    let code_hash      = EncryptedFieldsOps::generate_hash(card_code);
    let device_id_hash = EncryptedFieldsOps::generate_hash(device_id);
    format!(
        "verify:{}:{}:{}:{}",
        &api_key_hash[..16],
        app_id,
        &code_hash[..16],
        &device_id_hash[..16]
    )
}

/// 命中缓存后的复核结论。
#[derive(Debug, PartialEq, Eq)]
enum CacheVerdict {
    /// 复核通过，缓存内容仍然可信
    TrustCache,
    /// 缓存已失效，原因明确：删缓存并返回这条消息
    Reject(String),
    /// 无法判定（查询出错、缓存缺关键字段）：删缓存并回落完整查询，绝不猜
    FallThrough,
}

/// 判断一条「验证成功」的缓存是否仍然成立。
///
/// `live` 是实时查回来的 `(cards.status, cards.expires_at, 该设备是否仍绑定该卡密)`；
/// `None` 表示卡密行已被删除。
///
/// 为什么要复核绑定关系：缓存 key 的 TTL 在每次命中时会被重置为 60s，
/// 客户端持续轮询就能让一条缓存长期存活。只校验 `cards.status` 是不够的 ——
/// 解绑设备只删 activations 行、不改 cards.status（卡密还有别的设备绑着时仍是 active），
/// 只看状态就会让「已解绑的设备」一直验证通过。
fn judge_cached_entry(
    live: Option<(String, Option<DateTime<Utc>>, bool)>,
    now: DateTime<Utc>,
) -> CacheVerdict {
    let (status, expires_at, activation_exists) = match live {
        Some(v) => v,
        None => return CacheVerdict::Reject("卡密不存在".to_string()),
    };

    // 与缓存未命中路径保持同一套状态语义，避免两条路径答案不一致
    match status.as_str() {
        "disabled" => return CacheVerdict::Reject("卡密已被禁用".to_string()),
        "expired"  => return CacheVerdict::Reject("卡密已过期".to_string()),
        "unused"   => return CacheVerdict::Reject("卡密尚未激活".to_string()),
        _ => {}
    }

    if !activation_exists {
        return CacheVerdict::Reject("此设备未绑定该卡密".to_string());
    }

    if let Some(exp) = expires_at {
        if now > exp {
            return CacheVerdict::Reject("卡密已过期".to_string());
        }
    }

    CacheVerdict::TrustCache
}

/// 验证卡密
/// 性能优化：使用 Redis 缓存验证结果（TTL=60s）。
/// 命中缓存时不再跳过全部数据库查询 —— 仍会做一次轻量复核（卡密状态 + 激活记录是否存在），
/// 否则解绑/禁用/过期都会在缓存窗口内被漏过去。异步后台更新 last_verified_at。
async fn verify(
    State(state): State<AppState>,
    Json(body): Json<VerifyRequest>,
) -> Json<Value> {
    // ── Redis 缓存 key：api_key + app_id + card_code + device_id 的组合哈希 ──
    let api_key_hash   = EncryptedFieldsOps::generate_hash(&body.api_key);
    let code_hash      = EncryptedFieldsOps::generate_hash(&body.card_code);
    let device_id_hash = EncryptedFieldsOps::generate_hash(&body.device_id);
    let cache_key = verify_cache_key(&body.api_key, body.app_id, &body.card_code, &body.device_id);

    let mut redis = state.redis.clone();

    // ── 缓存命中：先实时复核，再返回缓存结果 ──
    // 缓存只保存「当时验证成功」的结果，但卡密可能在缓存期间被禁用/过期/解绑，
    // 因此命中时仍要做一次轻量复核（主键 + idx_activations_card_device_hash，开销极小）。
    if let Ok(Some(cached)) = redis.get::<_, Option<String>>(&cache_key).await {
        if let Ok(val) = serde_json::from_str::<Value>(&cached) {
            let cached_valid = val.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);

            // 失败缓存（例如「无效的 API Key」，TTL 只有 5s）不带 card_id，
            // 复核不了也不需要复核，原样返回 —— 它本身就是挡住暴力枚举的那层防护。
            if !cached_valid {
                return Json(val);
            }

            // 成功缓存必须能被复核。取不到 card_id 就视为不可信，
            // 删掉并回落完整查询：宁可多查一次，也不返回没复核过的结果。
            let cached_card_id = val.pointer("/data/card_id")
                .and_then(|v| v.as_str())
                .and_then(|s| uuid::Uuid::parse_str(s).ok());

            if let Some(cid) = cached_card_id {
                // 一次查询拿齐复核依据：卡密状态 + 到期时间 + 该设备是否仍绑定该卡密。
                //
                // 绑定的判定条件与缓存未命中路径**逐字一致**（card_id + device_id_hash），
                // 而不是拿缓存里的 activation_id 去比对 —— 解绑后重新激活会生成新的
                // activation_id，比 id 会把「刚重新绑好的设备」误判成未绑定。
                // 顺带把当前的 activation_id 取回来，供后台更新 last_verified_at 用，
                // 避免去写一个已经不存在的行。
                let (verdict, live_activation_id) =
                    match sqlx::query_as::<_, (String, Option<DateTime<Utc>>, Option<Uuid>)>(
                        "SELECT c.status, c.expires_at,
                            (SELECT a.id FROM activations a
                              WHERE a.card_id = c.id AND a.device_id_hash = $2)
                     FROM cards c WHERE c.id = $1",
                    )
                    .bind(cid)
                    .bind(&device_id_hash)
                    .fetch_optional(&state.pool)
                    .await
                    {
                        // 顺手把当前 activation_id 带出来，复核通过时用它更新 last_verified_at
                        Ok(Some((status, expires_at, act_id))) => {
                            let exists = act_id.is_some();
                            (
                                judge_cached_entry(Some((status, expires_at, exists)), Utc::now()),
                                act_id,
                            )
                        }
                        Ok(None) => (judge_cached_entry(None, Utc::now()), None),
                        Err(e) => {
                            // 查询出错时不要猜「卡密不存在」—— 那会把一次数据库抖动
                            // 变成用户的卡密失效。删掉缓存回落完整查询，让错误在那边被处理。
                            tracing::error!("[verify] 缓存复核查询失败，回落完整查询: {}", e);
                            (CacheVerdict::FallThrough, None)
                        }
                    };

                match verdict {
                    CacheVerdict::TrustCache => {
                        // 复核通过，使用缓存结果并异步更新 last_verified_at
                        let pool_bg      = state.pool.clone();
                        let mut redis_bg = state.redis.clone();
                        let cache_key_bg = cache_key.clone();
                        tokio::spawn(async move {
                            if let Some(act_id) = live_activation_id {
                                // 「最后验证时间」只是运维指标，写不进去不影响本次结论，
                                // 但静默会让这个指标悄悄失真（看起来是「这段时间没人验证」）
                                if let Err(e) = sqlx::query(
                                    "UPDATE activations SET last_verified_at = NOW() WHERE id = $1",
                                )
                                .bind(act_id)
                                .execute(&pool_bg)
                                .await
                                {
                                    tracing::warn!(
                                        "更新激活记录的最后验证时间失败（缓存命中路径）: activation_id={} err={}",
                                        act_id,
                                        e
                                    );
                                }
                            }
                            // 续期是有意的：命中路径每次都会重新复核激活记录，
                            // 所以「缓存活着」不再等于「结果没被复核」。
                            // 纯粹的优化动作：续不上只是下次多回源一次，结论不受影响
                            redis_guard::best_effort::<()>(
                                redis_bg.expire(cache_key_bg.as_str(), 60_i64).await,
                                "续期 verify 缓存 TTL",
                            );
                        });
                        return Json(val);
                    }
                    CacheVerdict::Reject(msg) => {
                        // 删缓存失败 → 这条陈旧条目会留到 TTL 到期；但命中路径每次都会
                        // 拿 DB 实时复核（见 judge_cached_entry），所以**结论仍然正确**。
                        // 属于派生态，best_effort。
                        redis_guard::best_effort::<()>(
                            redis.del(&cache_key).await,
                            "删除被否决的 verify 缓存",
                        );
                        return Json(json!({"success": false, "valid": false, "message": msg}));
                    }
                    CacheVerdict::FallThrough => {
                        redis_guard::best_effort::<()>(
                            redis.del(&cache_key).await,
                            "删除无法复核的 verify 缓存",
                        );
                    }
                }
            } else {
                // 缓存里缺关键字段，无法复核 → 删掉，走完整查询
                redis_guard::best_effort::<()>(
                    redis.del(&cache_key).await,
                    "删除字段不全的 verify 缓存",
                );
            }
        }
    }

    // ── 缓存未命中：走完整数据库查询逻辑 ──
    //
    // ⚠️ 两件事在这里曾经是错的：
    //   1. `.unwrap_or(None)` 把数据库故障伪装成「无效的 API Key」；
    //   2. 更糟的是**那个错误的「无效」结论会被写进缓存 5 秒** ——
    //      数据库抖一次，所有商户在接下来 5 秒内即使数据库恢复了也继续被拒，
    //      而且缓存里存的是一份**基于故障得出的判定**。故障被放大并留下了痕迹。
    //
    // 所以现在只有「确认查不到」才写缓存；`Failed` 直接返回服务繁忙、不写缓存。
    let merchant = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'",
        )
        .bind(&api_key_hash)
        .fetch_optional(&state.pool),
        "按 API Key 查询商户（verify）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(m) => m,
        db_guard::QueryOutcome::NotFound => {
            // 确认无效 —— 可以缓存，防止暴力枚举打穿数据库
            //
            // ⚠️ 这是**防护措施**而不是普通缓存：写不进去意味着「这条无效 Key 的结论
            // 没被记住」，于是每一次尝试都要回源查库 —— 也就是说，**防枚举在 Redis
            // 写不进的这段时间里被完全撤掉了**。功能不受影响（结论仍然正确），
            // 所以放行；但必须用 error 级留痕，否则防护消失是无声的。
            let fail = json!({"success": false, "valid": false, "message": "无效的 API Key"});
            redis_guard::protection_lost::<()>(
                redis::AsyncCommands::set_ex(
                    &mut redis, &cache_key, fail.to_string(), 5_u64,
                ).await,
                "写入无效 API Key 负缓存（防枚举）",
            );
            return Json(fail);
        }
        // 查询失败：**不缓存**。否则一个瞬时故障会被固化成 5 秒的「确定结论」。
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let merchant_id = merchant.0;

    match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'",
        )
        .bind(body.app_id)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "校验应用归属（verify）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(_) => {}
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "应用不存在或已禁用"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    }

    // ⚠️ 曾用 `.unwrap_or(None)` —— 数据库抖动会变成「卡密不存在」。
    // 用户拿着正确的卡密却被告知不存在，而日志里没有任何异常。
    let card = match db_guard::optional(
        sqlx::query_as::<_, Card>(
            "SELECT * FROM cards WHERE code_hash = $1 AND merchant_id = $2 AND app_id = $3",
        )
        .bind(&code_hash)
        .bind(merchant_id)
        .bind(body.app_id)
        .fetch_optional(&state.pool),
        "查询卡密",
    )
    .await
    {
        db_guard::QueryOutcome::Found(c) => c,
        // 确认查不到 —— 正常的业务结论
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "卡密不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    // 检查时间过期
    if let Some(exp) = card.expires_at {
        if Utc::now() > exp {
            // 这次标记只是把 status 快照落库；本次响应按内存里的 expires_at 判定，
            // 写不写都是「已过期」，所以失败不影响结论 —— 归到「可降级」类。
            // 但**不能静默**：否则「卡密状态列一直显示 active，接口却说已过期」
            // 会变成一个没人知道从哪查起的不一致。
            if let Err(e) = sqlx::query("UPDATE cards SET status = 'expired' WHERE id = $1")
                .bind(card.id)
                .execute(&state.pool)
                .await
            {
                tracing::warn!(
                    "标记卡密过期状态失败（本次响应不受影响，仍按 expires_at 判定为已过期）: card_id={} err={}",
                    card.id,
                    e
                );
            }
            return Json(json!({"success": false, "message": "卡密已过期", "valid": false}));
        }
    }

    match card.status.as_str() {
        "disabled" => return Json(json!({"success": false, "valid": false, "message": "卡密已被禁用"})),
        "expired"  => return Json(json!({"success": false, "valid": false, "message": "卡密已过期"})),
        "unused"   => return Json(json!({"success": false, "valid": false, "message": "卡密尚未激活"})),
        _ => {}
    }

    // ⚠️ 曾用 `.unwrap_or(None)` + `.unwrap()` 两步走。除了同一个静默失败问题，
    // 那个 `.unwrap()` 还让编译器无法证明这里有值 —— 万一将来有人把
    // `if activation.is_none() { return }` 挪走或加条件，`.unwrap()` 就会 panic。
    // 现在一次 match 把两种情况分清楚，没有 unwrap。
    let activation = match db_guard::optional(
        sqlx::query_as::<_, Activation>(
            "SELECT * FROM activations WHERE card_id = $1 AND device_id_hash = $2",
        )
        .bind(card.id)
        .bind(&device_id_hash)
        .fetch_optional(&state.pool),
        "查询设备激活记录（verify）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(a) => a,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({
                "success": false,
                "valid": false,
                "message": "此设备未绑定该卡密"
            }))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    // 异步更新最后验证时间（不阻塞响应）
    // 这是运维指标，失败不影响本次 verify 的结论（有效性上面已判定完），
    // 但静默会让指标悄悄失真，所以记 warn。
    let pool_bg = state.pool.clone();
    let act_id  = activation.id;
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE activations SET last_verified_at = NOW() WHERE id = $1",
        )
        .bind(act_id)
        .execute(&pool_bg)
        .await
        {
            tracing::warn!(
                "更新激活记录的最后验证时间失败: activation_id={} err={}",
                act_id,
                e
            );
        }
    });

    let remaining_days = card.expires_at.map(|e| (e - Utc::now()).num_days().max(0) + 1);
    // `current_devices` 只是给客户端的展示信息，不参与任何判定。
    //
    // 这一处归到「可降级」类，不是「必须报错」类：卡密的有效性在上面的激活记录查询里
    // 已经确认过了，因为一个展示字段查不出来就返回 503，会把一次正常的 verify 变成失败，
    // 客户端可能因此把用户挡在门外 —— 那比显示一个陈旧的计数更糟。
    //
    // 但也**不能**静默地当成 0：0 是个看起来完全合理的数字（新卡就是 0），
    // 所以调用方分辨不出「真的没设备」和「没查到」。这里用 warn 留痕，
    // 并降级为 `None` 让它序列化成 null —— 让「不确定」在数据里可见。
    let device_count = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM activations WHERE card_id = $1")
            .bind(card.id)
            .fetch_one(&state.pool),
        "统计卡密已绑定设备数（verify 展示字段）",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(c) => Some(c.0),
        db_guard::ScalarOutcome::Failed => None,
    };

    let result = json!({
        "success": true,
        "valid": true,
        "message": "卡密有效",
        "data": {
            "activation_id": activation.id,
            "card_id": card.id,
            "expires_at": card.expires_at,
            "remaining_days": remaining_days,
            "max_devices": card.max_devices,
            // 查不出来时为 null（而不是 0）—— 见上面的注释
            "current_devices": device_count
        }
    });

    // 写入缓存：60s TTL。
    // 失效策略：不依赖「每个写入方都记得来删缓存」—— 那种约定漏一个就是一个静默 bug
    // （解绑曾经就漏了）。命中路径会实时复核卡密状态和激活记录（见 judge_cached_entry），
    // 所以禁用/过期/解绑都能在下次 verify 立即生效；unbind 额外主动删一次是为了省掉那次复核。
    // 写不进只是下次回源查库（派生态），但**必须留痕** —— 否则「缓存为什么一直不命中」查不到原因。
    redis_guard::best_effort::<()>(
        redis::AsyncCommands::set_ex(
            &mut redis, &cache_key, result.to_string(), 60_u64,
        ).await,
        "写入 verify 结果缓存",
    );

    // 异步触发 Webhook（verify 事件）
    let pool_clone = state.pool.clone();
    let app_id_clone = card.app_id;
    let webhook_payload = serde_json::json!({
        "card_code": body.card_code,
        "device_id": body.device_id,
        "expires_at": card.expires_at,
        "remaining_days": remaining_days,
    });
    tokio::spawn(async move {
        crate::routes::webhooks::fire_webhook(&pool_clone, app_id_clone, "verify", webhook_payload).await;
    });

    Json(result)
}

/// 解绑设备
async fn unbind(
    State(state): State<AppState>,
    Json(body): Json<UnbindRequest>,
) -> Json<Value> {
    // 查询所有商户并使用哈希索引查询 API Key
    //
    // ⚠️ 这里曾用 `.unwrap_or(None)` —— 后果是**数据库一抖，所有商户同时收到
    // 「无效的 API Key」**，而日志上一个字都没有。排查的人会去查密钥，
    // 真正的问题却在数据库。现在把「查不到」和「查不了」分开。
    let api_key_hash = EncryptedFieldsOps::generate_hash(&body.api_key);
    let merchant = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'",
        )
        .bind(&api_key_hash)
        .fetch_optional(&state.pool),
        "按 API Key 查询商户",
    )
    .await
    {
        db_guard::QueryOutcome::Found(m) => m,
        // 查通了但没有这个 key —— 正常的业务结论
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "无效的 API Key"}))
        }
        // 查询失败 —— 是故障，不能伪装成「key 无效」
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let merchant_id = merchant.0;

    // 验证 app_id 归属于该商户且处于 active 状态
    match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'",
        )
        .bind(body.app_id)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "校验应用归属",
    )
    .await
    {
        db_guard::QueryOutcome::Found(_) => {}
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "应用不存在或已禁用"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    }

    // 查询该商户指定应用下的卡密（使用哈希索引查询）
    let code_hash = EncryptedFieldsOps::generate_hash(&body.card_code);
    // ⚠️ 曾用 `.unwrap_or(None)` —— 数据库抖动会变成「卡密不存在」。
    // 用户拿着正确的卡密却被告知不存在，而日志里没有任何异常。
    let card = match db_guard::optional(
        sqlx::query_as::<_, Card>(
            "SELECT * FROM cards WHERE code_hash = $1 AND merchant_id = $2 AND app_id = $3",
        )
        .bind(&code_hash)
        .bind(merchant_id)
        .bind(body.app_id)
        .fetch_optional(&state.pool),
        "查询卡密",
    )
    .await
    {
        db_guard::QueryOutcome::Found(c) => c,
        // 确认查不到 —— 正常的业务结论
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "卡密不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let card_id = card.id;

    // 查询该卡密的激活记录（使用哈希索引查询）
    //
    // ⚠️ 这里曾用 `.unwrap_or(None)`。要注意「设备未绑定该卡密」是**正常业务结论**
    // （用户输错设备号、或本来就没绑过），必须原样保留；而数据库查询失败是另一回事，
    // 不能借这句话混过去 —— 否则运维看到的是「用户老输错」，实际是数据库在抖。
    let device_id_hash = EncryptedFieldsOps::generate_hash(&body.device_id);
    let activation_id = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM activations WHERE card_id = $1 AND device_id_hash = $2",
        )
        .bind(card_id)
        .bind(&device_id_hash)
        .fetch_optional(&state.pool),
        "查询设备激活记录（unbind）",
    )
    .await
    {
        db_guard::QueryOutcome::Found((id,)) => id,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "设备未绑定该卡密"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let result = sqlx::query(
        "DELETE FROM activations WHERE id = $1",
    )
    .bind(activation_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            // 若无剩余设备，恢复卡密状态。
            //
            // ⚠️ 这里曾用 `.unwrap_or((0,))`，而它的失败方向恰好**最坏**：
            // 计数查不出来 → 当成 0 → 判定「这台设备是最后一个」→ 把卡密恢复成 `unused`。
            // 后果是：一张明明还被别的设备绑着的卡，状态被改回未使用 —— 这是**放松**方向，
            // 会让「一卡多绑」的重复激活检测失效。
            //
            // 注意这里**不能像别处那样整单拒绝**：上面那条 DELETE 已经执行成功了，
            // 此时返回「服务器繁忙」是撒谎（解绑其实做成了），客户端重试反而会得到
            // 「设备未绑定该卡密」。所以正确的取舍是：**跳过状态恢复**（宁可卡密状态偏保守地
            // 停在已激活），并 error 留痕让人能去库上手工确认。
            match db_guard::scalar(
                sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM activations WHERE card_id = $1")
                    .bind(card_id)
                    .fetch_one(&state.pool),
                "统计卡密剩余激活数（unbind 收尾）",
            )
            .await
            {
                db_guard::ScalarOutcome::Found(remaining) => {
                    if remaining.0 == 0 {
                        // ⚠️ 这里曾经是 `let _ =`，写失败没有任何痕迹。
                        // 后果：卡密状态停在 active（界面上显示「已激活 / 在用」），
                        // 而这台设备其实已经解绑了 —— 一个没人会发现的不一致。
                        //
                        // `AND status = 'active'` 是顺带修掉的另一个问题（原本缺这个守卫）：
                        // `disabled` 是**商户主动设置**的状态（cards.rs 的禁用卡密），
                        // 商户套餐降级时也会把卡密置为 disabled（workers/downgrade.rs）。
                        // 没有守卫时，用户解绑最后一台设备就会把一张被主动禁用的卡
                        // 复位成 `unused` —— 等于解绑把禁用操作悄悄撤销了。
                        // activations.rs 的同名收尾一直带这个守卫，这里漏了。
                        if let Err(e) = sqlx::query(
                            "UPDATE cards SET status = 'unused', activated_at = NULL, expires_at = NULL WHERE id = $1 AND status = 'active'",
                        )
                        .bind(card_id)
                        .execute(&state.pool)
                        .await
                        {
                            tracing::error!(
                                "解绑已完成，但恢复卡密状态失败（卡密会停在 active，需人工确认）: card_id={} err={}",
                                card_id,
                                e
                            );
                        }
                    }
                }
                db_guard::ScalarOutcome::Failed => {
                    tracing::error!(
                        "解绑已完成，但剩余激活数统计失败，已跳过卡密状态恢复: card_id={}",
                        card_id
                    );
                }
            }

            // 主动失效该设备的 verify 缓存。
            // 不删也不会返回错误结果（命中路径会复核激活记录），但那要多欠一次 DB 查询，
            // 而且这里 key 的四个组成部分全都拿得到，顺手删掉是零成本的。
            // 删不掉就是多欠一次复核 —— 派生态，留痕即可。
            let mut redis = state.redis.clone();
            let cache_key = verify_cache_key(
                &body.api_key, body.app_id, &body.card_code, &body.device_id,
            );
            redis_guard::best_effort::<()>(
                redis.del(&cache_key).await,
                "解绑后主动失效 verify 缓存",
            );

            Json(json!({"success": true, "message": "设备已解绑"}))
        }
        Ok(_) => Json(json!({"success": false, "message": "设备未绑定该卡密"})),
        // 原先是 `format!("操作失败: {}", e)` —— sqlx 原文外泄。
        // 注意 `Ok(_)`（0 行，业务结论「未绑定」）与 `Err`（查不了）必须分开。
        Err(e) => db_guard::internal_error("设备解绑", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, 0, 0, 0).unwrap()
    }

    fn app() -> Uuid {
        Uuid::parse_str("cc8d23a9-4bd3-47ca-b5f1-c086a3c02049").unwrap()
    }

    // ────────────── verify_cache_key ──────────────

    #[test]
    fn cache_key_is_stable_and_shaped() {
        let k1 = verify_cache_key("km_abc", app(), "KAMI-TALB-DUK3", "device-1");
        let k2 = verify_cache_key("km_abc", app(), "KAMI-TALB-DUK3", "device-1");
        assert_eq!(k1, k2, "同样输入必须得到同样的 key —— 否则主动失效会静默删不到");

        let parts: Vec<&str> = k1.split(':').collect();
        assert_eq!(parts.len(), 5, "key 应形如 verify:{{16}}:{{app}}:{{16}}:{{16}}");
        assert_eq!(parts[0], "verify");
        assert_eq!(parts[2], "cc8d23a9-4bd3-47ca-b5f1-c086a3c02049");
        assert_eq!(parts[1].len(), 16);
        assert_eq!(parts[3].len(), 16);
        assert_eq!(parts[4].len(), 16);
    }

    #[test]
    fn cache_key_differs_by_every_component() {
        let base = verify_cache_key("km_abc", app(), "CARD", "dev");
        let other_app = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert_ne!(base, verify_cache_key("km_xyz", app(), "CARD", "dev"), "换 api_key 应换 key");
        assert_ne!(base, verify_cache_key("km_abc", other_app, "CARD", "dev"), "换 app 应换 key");
        assert_ne!(base, verify_cache_key("km_abc", app(), "CARD2", "dev"), "换卡密应换 key");
        assert_ne!(base, verify_cache_key("km_abc", app(), "CARD", "dev2"), "换设备应换 key");
    }

    // ────────────── judge_cached_entry ──────────────

    #[test]
    fn cached_entry_rejected_when_activation_unbound() {
        // 核心回归用例。解绑只删 activations 行，而卡密状态可能仍是 active
        // （还有别的设备绑着），所以只看 status 是拦不住已解绑设备的。
        assert_eq!(
            judge_cached_entry(Some(("active".to_string(), Some(at(2030, 1, 1)), false)), at(2026, 9, 22)),
            CacheVerdict::Reject("此设备未绑定该卡密".to_string())
        );
    }

    #[test]
    fn cached_entry_rejected_when_card_gone() {
        assert_eq!(
            judge_cached_entry(None, at(2026, 9, 22)),
            CacheVerdict::Reject("卡密不存在".to_string())
        );
    }

    #[test]
    fn cached_entry_rejects_non_active_statuses() {
        let now = at(2026, 9, 22);
        let exp = Some(at(2030, 1, 1));
        for (status, msg) in [
            ("disabled", "卡密已被禁用"),
            ("expired", "卡密已过期"),
            ("unused", "卡密尚未激活"), // 解绑最后一台设备后卡密会被改回 unused
        ] {
            assert_eq!(
                judge_cached_entry(Some((status.to_string(), exp, true)), now),
                CacheVerdict::Reject(msg.to_string()),
                "状态 {} 必须被拒绝", status
            );
        }
    }

    #[test]
    fn cached_entry_rejects_expired_by_time() {
        assert_eq!(
            judge_cached_entry(Some(("active".to_string(), Some(at(2026, 1, 1)), true)), at(2026, 9, 22)),
            CacheVerdict::Reject("卡密已过期".to_string())
        );
    }

    #[test]
    fn cached_entry_trusted_when_everything_ok() {
        assert_eq!(
            judge_cached_entry(Some(("active".to_string(), Some(at(2030, 1, 1)), true)), at(2026, 9, 22)),
            CacheVerdict::TrustCache
        );
    }

    #[test]
    fn cached_entry_trusted_for_never_expiring_card() {
        assert_eq!(
            judge_cached_entry(Some(("active".to_string(), None, true)), at(2026, 9, 22)),
            CacheVerdict::TrustCache
        );
    }
}
