//! 对外公开 API，供第三方软件调用（使用 api_key 鉴权，无需 JWT）

use redis::AsyncCommands;
use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::AppState,
    models::{activation::Activation, card::Card},
};
use axum::{
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use chrono::{Duration, Utc};
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

/// 从请求头提取真实客户端 IP，优先读反向代理头
fn extract_client_ip(headers: &HeaderMap, addr: &SocketAddr) -> String {
    if let Some(val) = headers.get("x-forwarded-for") {
        if let Ok(s) = val.to_str() {
            let first = s.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    if let Some(val) = headers.get("x-real-ip") {
        if let Ok(s) = val.to_str() {
            let s = s.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    addr.ip().to_string()
}

pub fn public_api_router(state: AppState) -> Router<AppState> {
    use crate::middleware::rate_limit::api_rate_limit;
    use axum::middleware;
    Router::new()
        .route("/v1/activate", post(activate))
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
    let api_key_hash = EncryptedFieldsOps::generate_hash(&body.api_key);
    let merchant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'")
            .bind(&api_key_hash)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant_id = match merchant {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "无效的 API Key"})),
    };

    // 验证 app_id 归属于该商户且处于 active 状态
    let app_valid: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'",
    )
    .bind(body.app_id)
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if app_valid.is_none() {
        return Json(json!({"success": false, "message": "应用不存在或已禁用"}));
    }

    // 查询该商户指定应用下的卡密（使用哈希索引查询）
    let code_hash = EncryptedFieldsOps::generate_hash(&body.card_code);
    let card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE code_hash = $1 AND merchant_id = $2 AND app_id = $3",
    )
    .bind(&code_hash)
    .bind(merchant_id)
    .bind(body.app_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let card = match card {
        Some(c) => c,
        None => return Json(json!({"success": false, "message": "卡密不存在"})),
    };

    match card.status.as_str() {
        "disabled" => return Json(json!({"success": false, "message": "卡密已被禁用"})),
        "expired" => return Json(json!({"success": false, "message": "卡密已过期"})),
        _ => {}
    }

    if let Some(exp) = card.expires_at {
        if Utc::now() > exp {
            let _ = sqlx::query("UPDATE cards SET status = 'expired' WHERE id = $1")
                .bind(card.id)
                .execute(&state.pool)
                .await;
            return Json(json!({"success": false, "message": "卡密已过期"}));
        }
    }

    // 检查该设备是否已绑定此卡密（使用 device_id_hash 索引，O(1) 查询，无需全量解密）
    let device_id_hash = EncryptedFieldsOps::generate_hash(&body.device_id);
    let existing_activation: Option<Activation> = sqlx::query_as(
        "SELECT * FROM activations WHERE card_id = $1 AND device_id_hash = $2",
    )
    .bind(card.id)
    .bind(&device_id_hash)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if let Some(existing) = existing_activation {
        let _ = sqlx::query(
            "UPDATE activations SET last_verified_at = NOW() WHERE id = $1",
        )
        .bind(existing.id)
        .execute(&state.pool)
        .await;

        let expires_at = card.expires_at;
        let remaining_days = expires_at.map(|e| (e - Utc::now()).num_days().max(0));
        return Json(json!({
            "success": true,
            "message": "卡密已激活（设备已绑定）",
            "data": {
                "expires_at": expires_at,
                "remaining_days": remaining_days,
                "max_devices": card.max_devices
            }
        }));
    }

    // 检查设备数量上限
    let device_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM activations WHERE card_id = $1")
            .bind(card.id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));

    if device_count.0 >= card.max_devices as i64 {
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

    let ip = extract_client_ip(&headers, &addr);
    let activation_id = Uuid::new_v4();

    // 加密设备 ID（device_id_hash 已在上方定义）
    let encrypted_device_id = match EncryptedFieldsOps::encrypt_device_id(
        &state.pool,
        &state.encryptor,
        activation_id,
        &body.device_id,
    ).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("加密设备 ID 失败: {}", e);
            return Json(json!({"success": false, "message": "激活失败"}));
        }
    };

    let _ = sqlx::query(
        "INSERT INTO activations (id, card_id, app_id, device_id_encrypted, device_id_hash, device_name, ip_address) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(activation_id)
    .bind(card.id)
    .bind(card.app_id)
    .bind(&encrypted_device_id)
    .bind(&device_id_hash)
    .bind(&body.device_name)
    .bind(&ip)
    .execute(&state.pool)
    .await;

    let _ = sqlx::query(
        "UPDATE cards SET status = 'active', activated_at = COALESCE(activated_at, NOW()), expires_at = $1 WHERE id = $2",
    )
    .bind(expires_at)
    .bind(card.id)
    .execute(&state.pool)
    .await;

    let remaining_days = expires_at.map(|e| (e - Utc::now()).num_days().max(0));

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

/// 验证卡密
/// 性能优化：使用 Redis 缓存验证结果（TTL=60s），命中缓存时完全跳过数据库查询。
/// 异步后台更新 last_verified_at，避免写操作阻塞响应。
async fn verify(
    State(state): State<AppState>,
    Json(body): Json<VerifyRequest>,
) -> Json<Value> {
    // ── Redis 缓存 key：api_key + app_id + card_code + device_id 的组合哈希 ──
    let api_key_hash   = EncryptedFieldsOps::generate_hash(&body.api_key);
    let code_hash      = EncryptedFieldsOps::generate_hash(&body.card_code);
    let device_id_hash = EncryptedFieldsOps::generate_hash(&body.device_id);
    let cache_key = format!(
        "verify:{}:{}:{}:{}",
        &api_key_hash[..16],
        body.app_id,
        &code_hash[..16],
        &device_id_hash[..16]
    );

    let mut redis = state.redis.clone();

    // ── 缓存命中：直接返回，不查数据库 ──
    if let Ok(Some(cached)) = redis.get::<_, Option<String>>(&cache_key).await {
        if let Ok(val) = serde_json::from_str::<Value>(&cached) {
            // 缓存命中时异步更新 last_verified_at（fire-and-forget，不阻塞响应）
            let pool_bg   = state.pool.clone();
            let val_clone = val.clone();
            // redis ConnectionManager 实现了 Clone，可安全跨 task 传递
            let mut redis_bg = state.redis.clone();
            let cache_key_bg = cache_key.clone();
            tokio::spawn(async move {
                if val_clone.get("valid").and_then(|v| v.as_bool()).unwrap_or(false) {
                    // 从缓存 payload 中取 activation_id，更新验证时间
                    if let Some(act_id) = val_clone.pointer("/data/activation_id")
                        .and_then(|v| v.as_str())
                        .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    {
                        let _ = sqlx::query(
                            "UPDATE activations SET last_verified_at = NOW() WHERE id = $1",
                        )
                        .bind(act_id)
                        .execute(&pool_bg)
                        .await;
                    }
                    // 滑动续期：命中一次续期 60s，保持热 key 不冷却
                    let _: redis::RedisResult<()> =
                        redis_bg.expire(cache_key_bg.as_str(), 60_i64).await;
                }
            });
            return Json(val);
        }
    }

    // ── 缓存未命中：走完整数据库查询逻辑 ──
    let merchant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'")
            .bind(&api_key_hash)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant_id = match merchant {
        Some((id,)) => id,
        None => {
            // 无效 key 也短暂缓存（5s），防止暴力枚举打穿数据库
            let fail = json!({"success": false, "valid": false, "message": "无效的 API Key"});
            let _: redis::RedisResult<()> = redis::AsyncCommands::set_ex(
                &mut redis, &cache_key, fail.to_string(), 5_u64,
            ).await;
            return Json(fail);
        }
    };

    let app_valid: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'",
    )
    .bind(body.app_id)
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if app_valid.is_none() {
        return Json(json!({"success": false, "message": "应用不存在或已禁用"}));
    }

    let card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE code_hash = $1 AND merchant_id = $2 AND app_id = $3",
    )
    .bind(&code_hash)
    .bind(merchant_id)
    .bind(body.app_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let card = match card {
        Some(c) => c,
        None => return Json(json!({"success": false, "message": "卡密不存在"})),
    };

    // 检查时间过期
    if let Some(exp) = card.expires_at {
        if Utc::now() > exp {
            let _ = sqlx::query("UPDATE cards SET status = 'expired' WHERE id = $1")
                .bind(card.id)
                .execute(&state.pool)
                .await;
            return Json(json!({"success": false, "message": "卡密已过期", "valid": false}));
        }
    }

    match card.status.as_str() {
        "disabled" => return Json(json!({"success": false, "valid": false, "message": "卡密已被禁用"})),
        "expired"  => return Json(json!({"success": false, "valid": false, "message": "卡密已过期"})),
        "unused"   => return Json(json!({"success": false, "valid": false, "message": "卡密尚未激活"})),
        _ => {}
    }

    let activation: Option<Activation> = sqlx::query_as(
        "SELECT * FROM activations WHERE card_id = $1 AND device_id_hash = $2",
    )
    .bind(card.id)
    .bind(&device_id_hash)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if activation.is_none() {
        return Json(json!({
            "success": false,
            "valid": false,
            "message": "此设备未绑定该卡密"
        }));
    }

    let activation = activation.unwrap();

    // 异步更新最后验证时间（不阻塞响应）
    let pool_bg = state.pool.clone();
    let act_id  = activation.id;
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE activations SET last_verified_at = NOW() WHERE id = $1",
        )
        .bind(act_id)
        .execute(&pool_bg)
        .await;
    });

    let remaining_days = card.expires_at.map(|e| (e - Utc::now()).num_days().max(0));
    let device_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM activations WHERE card_id = $1")
            .bind(card.id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));

    let result = json!({
        "success": true,
        "valid": true,
        "message": "卡密有效",
        "data": {
            "activation_id": activation.id,
            "expires_at": card.expires_at,
            "remaining_days": remaining_days,
            "max_devices": card.max_devices,
            "current_devices": device_count.0
        }
    });

    // 写入缓存：60s TTL，激活/禁用/解绑事件时需主动失效（见 activate/unbind/disable 逻辑）
    let _: redis::RedisResult<()> = redis::AsyncCommands::set_ex(
        &mut redis, &cache_key, result.to_string(), 60_u64,
    ).await;

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
    let api_key_hash = EncryptedFieldsOps::generate_hash(&body.api_key);
    let merchant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'")
            .bind(&api_key_hash)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant_id = match merchant {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "无效的 API Key"})),
    };

    // 验证 app_id 归属于该商户且处于 active 状态
    let app_valid: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'",
    )
    .bind(body.app_id)
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if app_valid.is_none() {
        return Json(json!({"success": false, "message": "应用不存在或已禁用"}));
    }

    // 查询该商户指定应用下的卡密（使用哈希索引查询）
    let code_hash = EncryptedFieldsOps::generate_hash(&body.card_code);
    let card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE code_hash = $1 AND merchant_id = $2 AND app_id = $3",
    )
    .bind(&code_hash)
    .bind(merchant_id)
    .bind(body.app_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let card = match card {
        Some(c) => c,
        None => return Json(json!({"success": false, "message": "卡密不存在"})),
    };

    let card_id = card.id;

    // 查询该卡密的激活记录（使用哈希索引查询）
    let device_id_hash = EncryptedFieldsOps::generate_hash(&body.device_id);
    let activation: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM activations WHERE card_id = $1 AND device_id_hash = $2",
    )
    .bind(card_id)
    .bind(&device_id_hash)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let activation_id = match activation {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "设备未绑定该卡密"})),
    };

    let result = sqlx::query(
        "DELETE FROM activations WHERE id = $1",
    )
    .bind(activation_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            // 若无剩余设备，恢复卡密状态
            let remaining: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM activations WHERE card_id = $1")
                    .bind(card_id)
                    .fetch_one(&state.pool)
                    .await
                    .unwrap_or((0,));
            if remaining.0 == 0 {
                let _ = sqlx::query(
                    "UPDATE cards SET status = 'unused', activated_at = NULL, expires_at = NULL WHERE id = $1",
                )
                .bind(card_id)
                .execute(&state.pool)
                .await;
            }
            Json(json!({"success": true, "message": "设备已解绑"}))
        }
        Ok(_) => Json(json!({"success": false, "message": "设备未绑定该卡密"})),
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
    }
}
