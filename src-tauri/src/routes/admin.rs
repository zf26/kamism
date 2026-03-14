use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::merchant::MerchantPublic,
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
}

pub fn admin_router_with_state(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/merchants", get(list_merchants))
        .route("/admin/merchants/:id/status", patch(update_merchant_status))
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

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1 OR email ILIKE $1",
    )
    .bind(&like)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    let merchants: Vec<crate::models::merchant::Merchant> = sqlx::query_as(
        "SELECT * FROM merchants WHERE username ILIKE $1 OR email ILIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(&like)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let public: Vec<MerchantPublic> = merchants.into_iter().map(Into::into).collect();
    Json(json!({
        "success": true,
        "data": public,
        "total": total.0,
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
