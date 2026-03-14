use crate::{
    middleware::auth::{AppState, auth_middleware},
    models::card::Card,
    utils::{card_gen::generate_card_code, jwt::Claims},
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{get, patch},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct GenerateCardsRequest {
    pub app_id: Uuid,
    pub count: u32,
    pub duration_days: i32,
    pub max_devices: i32,
    pub note: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct CardQuery {
    pub app_id: Option<Uuid>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub fn cards_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/cards", get(list_cards).post(generate_cards))
        .route("/cards/:id", get(get_card).delete(delete_card))
        .route("/cards/:id/disable", patch(disable_card))
        .route("/cards/:id/enable", patch(enable_card))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<CardQuery>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let (cards, total): (Vec<Card>, i64) = if claims.role == "admin" {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cards")
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));
        let cards: Vec<Card> = sqlx::query_as(
            "SELECT * FROM cards ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (cards, total.0)
    } else {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM cards WHERE merchant_id = $1",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let cards: Vec<Card> = sqlx::query_as(
            "SELECT * FROM cards WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(merchant_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (cards, total.0)
    };

    Json(json!({
        "success": true,
        "data": cards,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

async fn generate_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<GenerateCardsRequest>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    if body.count == 0 || body.count > 1000 {
        return Json(json!({"success": false, "message": "生成数量需在 1-1000 之间"}));
    }
    if body.duration_days <= 0 {
        return Json(json!({"success": false, "message": "有效天数必须大于0"}));
    }
    if body.max_devices <= 0 || body.max_devices > 100 {
        return Json(json!({"success": false, "message": "设备数量需在 1-100 之间"}));
    }

    // 验证 app 归属
    let app_exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM apps WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin') AND status = 'active'",
    )
    .bind(body.app_id)
    .bind(merchant_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if app_exists.is_none() {
        return Json(json!({"success": false, "message": "应用不存在或已禁用"}));
    }

    let mut generated = 0u32;
    for _ in 0..body.count {
        let code = generate_card_code();
        let _ = sqlx::query(
            "INSERT INTO cards (app_id, merchant_id, code, duration_days, max_devices, note) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(body.app_id)
        .bind(merchant_id)
        .bind(&code)
        .bind(body.duration_days)
        .bind(body.max_devices)
        .bind(&body.note)
        .execute(&state.pool)
        .await;
        generated += 1;
    }

    Json(json!({
        "success": true,
        "message": format!("成功生成 {} 张卡密", generated),
        "count": generated
    }))
}

async fn get_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin')",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    match card {
        Some(c) => Json(json!({"success": true, "data": c})),
        None => Json(json!({"success": false, "message": "卡密不存在"})),
    }
}

async fn delete_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "DELETE FROM cards WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin') AND status = 'unused'",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "删除成功"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、已使用或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

async fn disable_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "UPDATE cards SET status = 'disabled' WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin')",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "卡密已禁用"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
    }
}

async fn enable_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    // 只允许将 disabled 状态的卡密重新启用为 unused
    let result = sqlx::query(
        "UPDATE cards SET status = 'unused' WHERE id = $1 AND status = 'disabled' AND (merchant_id = $2 OR $3 = 'admin')",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "卡密已启用"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、状态不符或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
    }
}
