use crate::{
    middleware::auth::{AppState, auth_middleware},
    models::app::App,
    utils::jwt::Claims,
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
pub struct CreateAppRequest {
    pub app_name: String,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct AppQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub fn apps_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/apps", get(list_apps).post(create_app))
        .route("/apps/:id", get(get_app).delete(delete_app))
        .route("/apps/:id/status", patch(update_app_status))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_apps(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<AppQuery>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let (apps, total): (Vec<App>, i64) = if claims.role == "admin" {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM apps")
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));
        let apps: Vec<App> = sqlx::query_as(
            "SELECT * FROM apps ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (apps, total.0)
    } else {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM apps WHERE merchant_id = $1",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let apps: Vec<App> = sqlx::query_as(
            "SELECT * FROM apps WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(merchant_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (apps, total.0)
    };

    Json(json!({
        "success": true,
        "data": apps,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

async fn create_app(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateAppRequest>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    if body.app_name.trim().is_empty() {
        return Json(json!({"success": false, "message": "应用名称不能为空"}));
    }

    let app: Result<App, _> = sqlx::query_as(
        "INSERT INTO apps (merchant_id, app_name, description) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(merchant_id)
    .bind(&body.app_name)
    .bind(&body.description)
    .fetch_one(&state.pool)
    .await;

    match app {
        Ok(a) => Json(json!({"success": true, "data": a})),
        Err(e) => Json(json!({"success": false, "message": format!("创建失败: {}", e)})),
    }
}

async fn get_app(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let app: Option<App> = sqlx::query_as(
        "SELECT * FROM apps WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin')",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    match app {
        Some(a) => Json(json!({"success": true, "data": a})),
        None => Json(json!({"success": false, "message": "应用不存在"})),
    }
}

async fn delete_app(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "DELETE FROM apps WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin')",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "删除成功"})),
        Ok(_) => Json(json!({"success": false, "message": "应用不存在或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

async fn update_app_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let status = match body.get("status").and_then(|s| s.as_str()) {
        Some(s) if s == "active" || s == "disabled" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效状态"})),
    };
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "UPDATE apps SET status = $1, updated_at = NOW() WHERE id = $2 AND (merchant_id = $3 OR $4 = 'admin')",
    )
    .bind(&status)
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "状态已更新"})),
        Ok(_) => Json(json!({"success": false, "message": "应用不存在或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

