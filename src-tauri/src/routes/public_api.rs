//! 对外公开 API，供第三方软件调用（使用 api_key 鉴权，无需 JWT）

use crate::{
    middleware::auth::AppState,
    models::{activation::Activation, card::Card},
};
use axum::{
    extract::{ConnectInfo, State},
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
    pub card_code: String,
    pub device_id: String,
    pub device_name: Option<String>,
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    pub api_key: String,
    pub card_code: String,
    pub device_id: String,
}

#[derive(Deserialize)]
pub struct UnbindRequest {
    pub api_key: String,
    pub card_code: String,
    pub device_id: String,
}

pub fn public_api_router(state: AppState) -> Router<AppState> {
    use crate::middleware::rate_limit::api_rate_limit;
    use axum::middleware;
    Router::new()
        .route("/api/v1/activate", post(activate))
        .route("/api/v1/verify", post(verify))
        .route("/api/v1/unbind", post(unbind))
        .route_layer(middleware::from_fn_with_state(state, api_rate_limit))
}

/// 激活卡密
async fn activate(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<ActivateRequest>,
) -> Json<Value> {
    if body.device_id.trim().is_empty() {
        return Json(json!({"success": false, "message": "设备ID不能为空"}));
    }

    let merchant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM merchants WHERE api_key = $1 AND status = 'active'")
            .bind(&body.api_key)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant_id = match merchant {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "无效的 API Key"})),
    };

    let card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE code = $1 AND merchant_id = $2",
    )
    .bind(&body.card_code)
    .bind(merchant_id)
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

    // 检查该设备是否已绑定此卡密
    let existing: Option<Activation> = sqlx::query_as(
        "SELECT * FROM activations WHERE card_id = $1 AND device_id = $2",
    )
    .bind(card.id)
    .bind(&body.device_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if existing.is_some() {
        let _ = sqlx::query(
            "UPDATE activations SET last_verified_at = NOW() WHERE card_id = $1 AND device_id = $2",
        )
        .bind(card.id)
        .bind(&body.device_id)
        .execute(&state.pool)
        .await;

        let expires_at = card.expires_at;
        let remaining_days = expires_at.map(|e| (e - Utc::now()).num_days().max(0));
        return Json(json!({
            "success": true,
            "message": "卡密已激活（设备已绑定）",
            "data": {
                "card_code": card.code,
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

    let ip = addr.ip().to_string();
    let _ = sqlx::query(
        "INSERT INTO activations (card_id, app_id, device_id, device_name, ip_address) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(card.id)
    .bind(card.app_id)
    .bind(&body.device_id)
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

    Json(json!({
        "success": true,
        "message": "激活成功",
        "data": {
            "card_code": card.code,
            "expires_at": expires_at,
            "remaining_days": remaining_days,
            "max_devices": card.max_devices,
            "current_devices": device_count.0 + 1
        }
    }))
}

/// 验证卡密
async fn verify(
    State(state): State<AppState>,
    Json(body): Json<VerifyRequest>,
) -> Json<Value> {
    let merchant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM merchants WHERE api_key = $1 AND status = 'active'")
            .bind(&body.api_key)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant_id = match merchant {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "无效的 API Key"})),
    };

    let card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE code = $1 AND merchant_id = $2",
    )
    .bind(&body.card_code)
    .bind(merchant_id)
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
        "expired" => return Json(json!({"success": false, "valid": false, "message": "卡密已过期"})),
        "unused" => return Json(json!({"success": false, "valid": false, "message": "卡密尚未激活"})),
        _ => {}
    }

    // 检查设备绑定
    let activation: Option<Activation> = sqlx::query_as(
        "SELECT * FROM activations WHERE card_id = $1 AND device_id = $2",
    )
    .bind(card.id)
    .bind(&body.device_id)
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

    // 更新最后验证时间
    let _ = sqlx::query(
        "UPDATE activations SET last_verified_at = NOW() WHERE card_id = $1 AND device_id = $2",
    )
    .bind(card.id)
    .bind(&body.device_id)
    .execute(&state.pool)
    .await;

    let remaining_days = card.expires_at.map(|e| (e - Utc::now()).num_days().max(0));
    let device_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM activations WHERE card_id = $1")
            .bind(card.id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));

    Json(json!({
        "success": true,
        "valid": true,
        "message": "卡密有效",
        "data": {
            "card_code": card.code,
            "expires_at": card.expires_at,
            "remaining_days": remaining_days,
            "max_devices": card.max_devices,
            "current_devices": device_count.0
        }
    }))
}

/// 解绑设备
async fn unbind(
    State(state): State<AppState>,
    Json(body): Json<UnbindRequest>,
) -> Json<Value> {
    let merchant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM merchants WHERE api_key = $1 AND status = 'active'")
            .bind(&body.api_key)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant_id = match merchant {
        Some((id,)) => id,
        None => return Json(json!({"success": false, "message": "无效的 API Key"})),
    };

    let card: Option<(Uuid, i32)> = sqlx::query_as(
        "SELECT id, max_devices FROM cards WHERE code = $1 AND merchant_id = $2",
    )
    .bind(&body.card_code)
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (card_id, _) = match card {
        Some(c) => c,
        None => return Json(json!({"success": false, "message": "卡密不存在"})),
    };

    let result = sqlx::query(
        "DELETE FROM activations WHERE card_id = $1 AND device_id = $2",
    )
    .bind(card_id)
    .bind(&body.device_id)
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
