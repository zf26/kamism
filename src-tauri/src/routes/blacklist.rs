//! 黑名单管理路由
//!
//! 商户接口（需 auth_middleware）：
//!   GET    /blacklist/ips            — IP 黑名单列表
//!   POST   /blacklist/ips            — 添加 IP
//!   DELETE /blacklist/ips/:id        — 删除 IP
//!   GET    /blacklist/devices        — 设备黑名单列表
//!   POST   /blacklist/devices        — 添加设备
//!   DELETE /blacklist/devices/:id    — 删除设备
//!   GET    /blacklist/alerts         — 异常告警列表
//!   POST   /blacklist/alerts/:id/read — 标记告警已读

use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{auth_middleware, AppState},
    utils::jwt::Claims,
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// ── 请求/响应结构 ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddIpRequest {
    pub ip: String,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct AddDeviceRequest {
    /// 明文 device_id，后端自动哈希存储
    pub device_id: String,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct IpBlacklistEntry {
    pub id: Uuid,
    pub ip: String,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct DeviceBlacklistEntry {
    pub id: Uuid,
    pub device_hint: Option<String>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct AlertEntry {
    pub id: Uuid,
    pub alert_type: String,
    pub card_id: Option<Uuid>,
    pub device_hint: Option<String>,
    pub ip_address: Option<String>,
    pub detail: Option<String>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

// ── 路由注册 ──────────────────────────────────────────────────────────────────

pub fn blacklist_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/blacklist/ips", get(list_ip).post(add_ip))
        .route("/blacklist/ips/:id", delete(remove_ip))
        .route("/blacklist/devices", get(list_device).post(add_device))
        .route("/blacklist/devices/:id", delete(remove_device))
        .route("/blacklist/alerts", get(list_alerts))
        .route("/blacklist/alerts/unread_count", get(alerts_unread_count))
        .route("/blacklist/alerts/:id/read", post(mark_alert_read))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

// ── IP 黑名单 ─────────────────────────────────────────────────────────────────

async fn list_ip(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PageQuery>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let rows: Vec<IpBlacklistEntry> = sqlx::query_as(
        "SELECT id, ip, reason, created_at FROM ip_blacklist
         WHERE merchant_id = $1
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(merchant_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM ip_blacklist WHERE merchant_id = $1"
    )
    .bind(merchant_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    Json(json!({
        "success": true,
        "data": rows,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

async fn add_ip(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<AddIpRequest>,
) -> Json<Value> {
    let ip = body.ip.trim().to_string();
    if ip.is_empty() {
        return Json(json!({"success": false, "message": "IP 不能为空"}));
    }
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "INSERT INTO ip_blacklist (merchant_id, ip, reason) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING"
    )
    .bind(merchant_id)
    .bind(&ip)
    .bind(&body.reason)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "已添加到 IP 黑名单"})),
        Ok(_) => Json(json!({"success": false, "message": "该 IP 已在黑名单中"})),
        Err(e) => Json(json!({"success": false, "message": format!("添加失败: {}", e)})),
    }
}

async fn remove_ip(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "DELETE FROM ip_blacklist WHERE id = $1 AND merchant_id = $2"
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "已移除"})),
        Ok(_) => Json(json!({"success": false, "message": "记录不存在或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

// ── 设备黑名单 ────────────────────────────────────────────────────────────────

async fn list_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PageQuery>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let rows: Vec<DeviceBlacklistEntry> = sqlx::query_as(
        "SELECT id, device_hint, reason, created_at FROM device_blacklist
         WHERE merchant_id = $1
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(merchant_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM device_blacklist WHERE merchant_id = $1"
    )
    .bind(merchant_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    Json(json!({
        "success": true,
        "data": rows,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

async fn add_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<AddDeviceRequest>,
) -> Json<Value> {
    let device_id = body.device_id.trim().to_string();
    if device_id.is_empty() {
        return Json(json!({"success": false, "message": "设备 ID 不能为空"}));
    }
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let device_id_hash = EncryptedFieldsOps::generate_hash(&device_id);
    let device_hint = if device_id.len() >= 4 {
        format!("{}****", &device_id[..4])
    } else {
        "****".to_string()
    };

    let result = sqlx::query(
        "INSERT INTO device_blacklist (merchant_id, device_id_hash, device_hint, reason)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING"
    )
    .bind(merchant_id)
    .bind(&device_id_hash)
    .bind(&device_hint)
    .bind(&body.reason)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "已添加到设备黑名单"})),
        Ok(_) => Json(json!({"success": false, "message": "该设备已在黑名单中"})),
        Err(e) => Json(json!({"success": false, "message": format!("添加失败: {}", e)})),
    }
}

async fn remove_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "DELETE FROM device_blacklist WHERE id = $1 AND merchant_id = $2"
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "已移除"})),
        Ok(_) => Json(json!({"success": false, "message": "记录不存在或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

// ── 异常告警 ──────────────────────────────────────────────────────────────────

async fn list_alerts(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PageQuery>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let rows: Vec<AlertEntry> = sqlx::query_as(
        "SELECT id, alert_type, card_id, device_hint, ip_address, detail, is_read, created_at
         FROM activation_alerts
         WHERE merchant_id = $1
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(merchant_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM activation_alerts WHERE merchant_id = $1"
    )
    .bind(merchant_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    Json(json!({
        "success": true,
        "data": rows,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

async fn alerts_unread_count(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM activation_alerts WHERE merchant_id = $1 AND is_read = FALSE"
    )
    .bind(merchant_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    Json(json!({"success": true, "data": {"unread": count.0}}))
}

async fn mark_alert_read(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "UPDATE activation_alerts SET is_read = TRUE WHERE id = $1 AND merchant_id = $2"
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "已标记已读"})),
        Ok(_) => Json(json!({"success": false, "message": "记录不存在或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
    }
}

