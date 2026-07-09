use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::payment_config::{PaymentConfig, PaymentConfigPublic, UpdatePaymentConfig},
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn payment_admin_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/payment/configs", get(list_payment_configs))
        .route("/admin/payment/configs/:channel", get(get_payment_config))
        .route("/admin/payment/configs/:channel", patch(update_payment_config))
        .route("/admin/payment/configs/:channel/toggle", post(toggle_payment_config))
        .route("/admin/payment/orders", get(list_all_orders))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
}

fn config_public(c: &PaymentConfig) -> PaymentConfigPublic {
    PaymentConfigPublic {
        id: c.id,
        channel: c.channel.clone(),
        name: c.name.clone(),
        enabled: c.enabled,
        alipay_app_id_set: c.alipay_app_id.as_ref().map_or(false, |s| !s.is_empty()),
        xorpay_aid_set: c.xorpay_aid.as_ref().map_or(false, |s| !s.is_empty()),
        mbdpay_app_id_set: c.mbdpay_app_id.as_ref().map_or(false, |s| !s.is_empty()),
    }
}

async fn list_payment_configs(State(state): State<AppState>) -> Json<Value> {
    let configs: Vec<PaymentConfig> = sqlx::query_as(
        "SELECT * FROM payment_configs ORDER BY channel",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let data: Vec<PaymentConfigPublic> = configs.iter().map(config_public).collect();

    Json(json!({
        "success": true,
        "data": data,
    }))
}

async fn get_payment_config(
    State(state): State<AppState>,
    Path(channel): Path<String>,
) -> Json<Value> {
    let config: Option<PaymentConfig> = sqlx::query_as(
        "SELECT * FROM payment_configs WHERE channel = $1",
    )
    .bind(&channel)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    match config {
        Some(c) => Json(json!({
            "success": true,
            "data": {
                "id": c.id,
                "channel": c.channel,
                "name": c.name,
                "enabled": c.enabled,
                "xorpay_aid": c.xorpay_aid,
                "xorpay_app_key_set": c.xorpay_app_key.as_ref().map_or(false, |s| !s.is_empty()),
                "xorpay_notify_url": c.xorpay_notify_url,
                "mbdpay_app_id": c.mbdpay_app_id,
                "mbdpay_app_key_set": c.mbdpay_app_key.as_ref().map_or(false, |s| !s.is_empty()),
                "mbdpay_notify_url": c.mbdpay_notify_url,
                "alipay_app_id": c.alipay_app_id,
                "alipay_private_key_set": c.alipay_private_key.as_ref().map_or(false, |s| !s.is_empty()),
                "alipay_public_key_set": c.alipay_public_key.as_ref().map_or(false, |s| !s.is_empty()),
                "alipay_notify_url": c.alipay_notify_url,
                "alipay_gateway": c.alipay_gateway,
                "alipay_return_url": c.alipay_return_url,
            }
        })),
        None => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
    }
}

async fn update_payment_config(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    Json(body): Json<UpdatePaymentConfig>,
) -> Json<Value> {
    let mut updates: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    macro_rules! add_update {
        ($field:literal, $value:expr) => {
            if $value.is_some() {
                updates.push(format!("{} = ${}", $field, params.len() + 1));
                params.push($value.unwrap());
            }
        };
    }

    add_update!("name", body.name);
    add_update!("xorpay_aid", body.xorpay_aid);
    add_update!("xorpay_app_key", body.xorpay_app_key);
    add_update!("xorpay_notify_url", body.xorpay_notify_url);
    add_update!("mbdpay_app_id", body.mbdpay_app_id);
    add_update!("mbdpay_app_key", body.mbdpay_app_key);
    add_update!("mbdpay_notify_url", body.mbdpay_notify_url);
    add_update!("alipay_app_id", body.alipay_app_id);
    add_update!("alipay_private_key", body.alipay_private_key);
    add_update!("alipay_public_key", body.alipay_public_key);
    add_update!("alipay_notify_url", body.alipay_notify_url);
    add_update!("alipay_gateway", body.alipay_gateway);
    add_update!("alipay_return_url", body.alipay_return_url);

    if updates.is_empty() {
        return Json(json!({
            "success": false,
            "message": "没有需要更新的字段"
        }));
    }

    updates.push("updated_at = NOW()".to_string());

    let query = format!(
        "UPDATE payment_configs SET {} WHERE channel = ${}",
        updates.join(", "),
        params.len() + 1
    );

    let mut q = sqlx::query(&query);
    for p in &params {
        q = q.bind(p);
    }
    q = q.bind(&channel);
    let result = q.execute(&state.pool).await;

    state.invalidate_payment_cache(Some(&channel)).await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "message": "配置已更新"
            }))
        }
        Ok(_) => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
        Err(e) => Json(json!({
            "success": false,
            "message": format!("更新失败: {}", e)
        })),
    }
}

async fn toggle_payment_config(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let enabled = match body.get("enabled").and_then(|v| v.as_bool()) {
        Some(v) => v,
        None => return Json(json!({
            "success": false,
            "message": "缺少 enabled 参数"
        })),
    };

    if enabled {
        // 启用前先禁用所有渠道（单选模式）
        sqlx::query("UPDATE payment_configs SET enabled = FALSE")
            .execute(&state.pool)
            .await
            .ok();
    }

    let result = sqlx::query(
        "UPDATE payment_configs SET enabled = $1, updated_at = NOW() WHERE channel = $2",
    )
    .bind(enabled)
    .bind(&channel)
    .execute(&state.pool)
    .await;

    state.invalidate_payment_cache(None).await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "message": if enabled { "已启用" } else { "已禁用" }
            }))
        }
        Ok(_) => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
        Err(e) => Json(json!({
            "success": false,
            "message": format!("操作失败: {}", e)
        })),
    }
}

// ── 订单管理 ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AdminOrdersQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub status: Option<String>,    // 筛选：paid / pending / expired / cancelled
    pub channel: Option<String>,   // 筛选：alipay / xorpay / mbdpay
}

type AdminOrderRow = (
    String,             // order_id
    String,             // merchant_id
    String,             // username
    String,             // pay_channel
    String,             // pay_type
    String,             // amount::text
    String,             // status
    Option<i32>,        // expires_days
    chrono::DateTime<chrono::Utc>, // created_at
    Option<chrono::DateTime<chrono::Utc>>, // pay_time
    Option<String>,     // pay_price::text
);

async fn list_all_orders(
    State(state): State<AppState>,
    Query(q): Query<AdminOrdersQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let orders: Vec<AdminOrderRow> = sqlx::query_as(
        r#"
        SELECT p.order_id, p.merchant_id::text, COALESCE(m.username, '(已删除)') AS username,
               p.pay_channel, p.pay_type, p.amount::text, p.status,
               p.expires_days, p.created_at, p.pay_time, p.pay_price::text
        FROM payments p
        LEFT JOIN merchants m ON m.id = p.merchant_id
        WHERE ($1::text IS NULL OR p.status = $1)
          AND ($2::text IS NULL OR p.pay_channel = $2)
        ORDER BY p.created_at DESC
        LIMIT $3 OFFSET $4
        "#
    )
    .bind(&q.status)
    .bind(&q.channel)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM payments WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR pay_channel = $2)"
    )
    .bind(&q.status)
    .bind(&q.channel)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    let data: Vec<Value> = orders.into_iter().map(|(
        order_id, merchant_id, username, pay_channel, pay_type,
        amount, status, expires_days, created_at, pay_time, pay_price,
    )| {
        json!({
            "order_id": order_id,
            "merchant_id": merchant_id,
            "username": username,
            "pay_channel": pay_channel,
            "pay_type": pay_type,
            "amount": amount,
            "status": status,
            "expires_days": expires_days,
            "created_at": created_at.to_rfc3339(),
            "pay_time": pay_time.map(|t| t.to_rfc3339()),
            "pay_price": pay_price,
        })
    }).collect();

    Json(json!({
        "success": true,
        "data": data,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}
