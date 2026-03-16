use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::merchant::MerchantPublic,
    utils::mq,
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{get, patch},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct MerchantQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub keyword: Option<String>,
    pub plan: Option<String>,
}

pub fn admin_router_with_state(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/merchants", get(list_merchants))
        .route("/admin/merchants/:id/status", patch(update_merchant_status))
        .route("/admin/merchants/:id/plan", patch(update_merchant_plan))
        .route("/admin/stats", get(get_stats))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_merchants(
    State(state): State<AppState>,
    Query(q): Query<MerchantQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;
    let keyword = q.keyword.unwrap_or_default();
    let like = format!("%{}%", keyword);
    let plan_filter = q.plan.as_deref().unwrap_or("");

    let (total, merchants) = if plan_filter.is_empty() {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1",
        )
        .bind(&like)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let rows: Vec<crate::models::merchant::Merchant> = sqlx::query_as(
            "SELECT * FROM merchants WHERE username ILIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(&like)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (total.0, rows)
    } else {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1 AND plan = $2",
        )
        .bind(&like)
        .bind(plan_filter)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let rows: Vec<crate::models::merchant::Merchant> = sqlx::query_as(
            "SELECT * FROM merchants WHERE username ILIKE $1 AND plan = $2 ORDER BY created_at DESC LIMIT $3 OFFSET $4",
        )
        .bind(&like)
        .bind(plan_filter)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (total.0, rows)
    };

    // 解密 email 和 api_key
    let public: Vec<MerchantPublic> = merchants.into_iter().map(|mut m| {
        if let Ok(plain) = EncryptedFieldsOps::decrypt_merchant_email(&state.encryptor, &m.email) {
            m.email = plain;
        } else {
            tracing::warn!("解密商户 {} email 失败", m.id);
        }
        if let Ok(plain) = EncryptedFieldsOps::decrypt_merchant_api_key(&state.encryptor, &m.api_key) {
            m.api_key = plain;
        } else {
            tracing::warn!("解密商户 {} api_key 失败", m.id);
        }
        m.into()
    }).collect();

    Json(json!({
        "success": true,
        "data": public,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

async fn update_merchant_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let status = match body.get("status").and_then(|s| s.as_str()) {
        Some(s) if s == "active" || s == "disabled" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效状态"})),
    };

    let result =
        sqlx::query("UPDATE merchants SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(&status)
            .bind(id)
            .execute(&state.pool)
            .await;

    match result {
        Ok(_) => Json(json!({"success": true, "message": "状态已更新"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

async fn update_merchant_plan(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let plan = match body.get("plan").and_then(|s| s.as_str()) {
        Some(s) if s == "free" || s == "pro" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效套餐，仅支持 free / pro"})),
    };

    // expires_days: 仅 pro 有效，None 表示永久，0 表示立即到期
    let expires_days = body.get("expires_days").and_then(|v| v.as_i64());

    let result = if plan == "pro" {
        match expires_days {
            Some(days) if days > 0 => {
                sqlx::query(
                    "UPDATE merchants
                     SET plan = $1,
                         plan_expires_at = NOW() + ($2 || ' days')::INTERVAL,
                         updated_at = NOW()
                     WHERE id = $3",
                )
                .bind(&plan)
                .bind(days.to_string())
                .bind(id)
                .execute(&state.pool)
                .await
            }
            _ => {
                // 永久专业版，清空到期时间
                sqlx::query(
                    "UPDATE merchants
                     SET plan = $1,
                         plan_expires_at = NULL,
                         updated_at = NOW()
                     WHERE id = $2",
                )
                .bind(&plan)
                .bind(id)
                .execute(&state.pool)
                .await
            }
        }
    } else {
        // 手动降为免费版，清空到期时间
        sqlx::query(
            "UPDATE merchants
             SET plan = $1,
                 plan_expires_at = NULL,
                 updated_at = NOW()
             WHERE id = $2",
        )
        .bind(&plan)
        .bind(id)
        .execute(&state.pool)
        .await
    };

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let msg = if plan == "pro" {
                // 升级为专业版：异步发布恢复消息
                if let Err(e) = mq::publish_upgrade(&state.mq_channel, &id.to_string()).await {
                    tracing::error!("发布升级恢复消息失败 {}: {}", id, e);
                }
                match expires_days {
                    Some(d) if d > 0 => format!("已升级为专业版，有效期 {} 天", d),
                    _ => "已升级为专业版（永久）".to_string(),
                }
            } else {
                "已降级为免费版".to_string()
            };
            Json(json!({"success": true, "message": msg}))
        }
        Ok(_) => Json(json!({"success": false, "message": "商户不存在"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

async fn get_stats(State(state): State<AppState>) -> Json<Value> {
    let merchant_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM merchants")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    let card_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cards")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    let activation_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM activations")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    let active_card_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM cards WHERE status = 'active'")
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));

    let app_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM apps")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    Json(json!({
        "success": true,
        "data": {
            "merchants": merchant_count.0,
            "total_cards": card_count.0,
            "active_cards": active_card_count.0,
            "total_activations": activation_count.0,
            "total_apps": app_count.0
        }
    }))
}
