use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::payment_config::{PaymentConfig, PaymentConfigPublic, UpdatePaymentConfig},
};
use axum::{
    extract::{Path, State},
    middleware,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::{json, Value};

pub fn payment_admin_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/payment/configs", get(list_payment_configs))
        .route("/admin/payment/configs/:channel", get(get_payment_config))
        .route("/admin/payment/configs/:channel", patch(update_payment_config))
        .route("/admin/payment/configs/:channel/toggle", post(toggle_payment_config))
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
                "xorpay_app_key": c.xorpay_app_key,
                "xorpay_notify_url": c.xorpay_notify_url,
                "mbdpay_app_id": c.mbdpay_app_id,
                "mbdpay_app_key": c.mbdpay_app_key,
                "mbdpay_notify_url": c.mbdpay_notify_url,
                "alipay_app_id": c.alipay_app_id,
                "alipay_private_key": c.alipay_private_key,
                "alipay_public_key": c.alipay_public_key,
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
