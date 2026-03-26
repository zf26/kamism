use crate::{
    middleware::auth::{AppState, auth_middleware},
    utils::jwt::Claims,
};
use axum::{
    extract::{Path, State},
    middleware,
    routing::{get},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct WebhookUpsertRequest {
    pub url: String,
    pub secret: Option<String>,
    pub enabled: Option<bool>,
    pub events: Option<Vec<String>>,
}

pub fn webhooks_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list_webhooks))
        .route("/webhooks/app/:app_id", get(get_webhook).put(upsert_webhook).delete(delete_webhook))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

/// 列出当前商户所有 Webhook 配置
async fn list_webhooks(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let rows: Vec<(Uuid, Uuid, String, String, bool, Vec<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, app_id, url, secret, enabled, events, created_at, updated_at
             FROM app_webhooks WHERE merchant_id = $1 ORDER BY created_at DESC",
        )
        .bind(merchant_id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let data: Vec<Value> = rows
        .into_iter()
        .map(|(id, app_id, url, secret, enabled, events, created_at, updated_at)| {
            json!({
                "id": id,
                "app_id": app_id,
                "url": url,
                "secret": mask_secret(&secret),
                "enabled": enabled,
                "events": events,
                "created_at": created_at,
                "updated_at": updated_at,
            })
        })
        .collect();

    Json(json!({ "success": true, "data": data }))
}

/// 获取指定应用的 Webhook 配置
async fn get_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(app_id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let row: Option<(Uuid, Uuid, String, String, bool, Vec<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, app_id, url, secret, enabled, events, created_at, updated_at
             FROM app_webhooks WHERE app_id = $1 AND merchant_id = $2",
        )
        .bind(app_id)
        .bind(merchant_id)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or(None);

    match row {
        Some((id, app_id, url, secret, enabled, events, created_at, updated_at)) => Json(json!({
            "success": true,
            "data": {
                "id": id,
                "app_id": app_id,
                "url": url,
                "secret": mask_secret(&secret),
                "enabled": enabled,
                "events": events,
                "created_at": created_at,
                "updated_at": updated_at,
            }
        })),
        None => Json(json!({ "success": false, "message": "未配置 Webhook" })),
    }
}

/// 创建或更新指定应用的 Webhook 配置（UPSERT）
async fn upsert_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(app_id): Path<Uuid>,
    Json(body): Json<WebhookUpsertRequest>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();

    if body.url.trim().is_empty() {
        return Json(json!({ "success": false, "message": "URL 不能为空" }));
    }
    if !body.url.starts_with("http://") && !body.url.starts_with("https://") {
        return Json(json!({ "success": false, "message": "URL 必须以 http:// 或 https:// 开头" }));
    }

    // 验证 app 归属
    let app_exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM apps WHERE id = $1 AND merchant_id = $2",
    )
    .bind(app_id)
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if app_exists.is_none() {
        return Json(json!({ "success": false, "message": "应用不存在或无权限" }));
    }

    // 若未提供 secret 则生成随机 32 字节 hex 字符串
    let secret = body.secret
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            // 使用随机 UUID hex 作为默认 secret
            format!("{}", Uuid::new_v4().simple())
        });

    let enabled = body.enabled.unwrap_or(true);
    let events = body.events.unwrap_or_else(|| vec!["activate".to_string(), "verify".to_string()]);

    // 验证 events
    for e in &events {
        if e != "activate" && e != "verify" {
            return Json(json!({ "success": false, "message": format!("不支持的事件类型：{}", e) }));
        }
    }

    let result = sqlx::query(
        "INSERT INTO app_webhooks (app_id, merchant_id, url, secret, enabled, events)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (app_id) DO UPDATE
         SET url = EXCLUDED.url,
             secret = CASE WHEN $4 = '' THEN app_webhooks.secret ELSE EXCLUDED.secret END,
             enabled = EXCLUDED.enabled,
             events = EXCLUDED.events,
             updated_at = NOW()",
    )
    .bind(app_id)
    .bind(merchant_id)
    .bind(&body.url)
    .bind(&secret)
    .bind(enabled)
    .bind(&events)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Json(json!({ "success": true, "message": "Webhook 配置已保存" })),
        Err(e) => Json(json!({ "success": false, "message": format!("保存失败: {}", e) })),
    }
}

/// 删除指定应用的 Webhook 配置
async fn delete_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(app_id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "DELETE FROM app_webhooks WHERE app_id = $1 AND merchant_id = $2",
    )
    .bind(app_id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({ "success": true, "message": "Webhook 已删除" })),
        Ok(_) => Json(json!({ "success": false, "message": "Webhook 不存在或无权限" })),
        Err(e) => Json(json!({ "success": false, "message": format!("删除失败: {}", e) })),
    }
}

/// 隐藏 secret 中间部分，只显示前4后4位
fn mask_secret(s: &str) -> String {
    if s.len() <= 8 {
        return "*".repeat(s.len());
    }
    format!("{}****{}", &s[..4], &s[s.len() - 4..])
}

/// 触发 Webhook 推送（由 public_api 调用，异步非阻塞）
pub async fn fire_webhook(
    pool: &sqlx::PgPool,
    app_id: Uuid,
    event: &str,
    payload: serde_json::Value,
) {
    let event = event.to_string(); // 转为 owned String，满足 tokio::spawn 的 'static 要求
    // 查询该应用是否有启用的 webhook 且包含该事件
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT url, secret FROM app_webhooks
         WHERE app_id = $1 AND enabled = TRUE AND $2 = ANY(events)",
    )
    .bind(app_id)
    .bind(&event)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    let (url, secret) = match row {
        Some(r) => r,
        None => return,
    };

    let timestamp = chrono::Utc::now().timestamp();
    let body = json!({
        "event": event,
        "timestamp": timestamp,
        "data": payload,
    })
    .to_string();

    // HMAC-SHA256 签名
    let signature = hmac_sha256_hex(&secret, &body);

    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        let res = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-KamiSM-Event", event.clone())
            .header("X-KamiSM-Signature", format!("sha256={}", signature))
            .header("X-KamiSM-Timestamp", timestamp.to_string())
            .body(body)
            .send()
            .await;
        match res {
            Ok(r) => tracing::info!("Webhook {} -> {} status={}", event, url, r.status()),
            Err(e) => tracing::warn!("Webhook {} -> {} failed: {}", event, url, e),
        }
    });
}

fn hmac_sha256_hex(key: &str, data: &str) -> String {
    use sha2::Sha256;
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap_or_else(|_| {
        HmacSha256::new_from_slice(b"fallback").unwrap()
    });
    mac.update(data.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

