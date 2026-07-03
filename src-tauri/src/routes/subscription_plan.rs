use crate::middleware::auth::{admin_only, auth_middleware, AppState};
use crate::models::subscription_plan::{CreatePlanRequest, SubscriptionPlan, UpdatePlanRequest};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{get, post, put, delete},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ListQuery {
    pub enabled_only: Option<bool>,
}

pub fn subscription_plan_router(state: AppState) -> Router<AppState> {
    Router::new()
        // 管理端接口（需管理员）
        .route("/admin/subscription-plans", get(list_plans))
        .route("/admin/subscription-plans", post(create_plan))
        .route("/admin/subscription-plans/:id", put(update_plan))
        .route("/admin/subscription-plans/:id", delete(delete_plan))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        // 商户端公开接口（仅需登录，无需管理员）
        .route("/pay/auth/plans", get(list_enabled_plans))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
}

async fn list_plans(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Json<Value> {
    let plans: Vec<SubscriptionPlan> = match q.enabled_only {
        Some(true) => {
            sqlx::query_as(
                "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
                 FROM subscription_plans WHERE enabled = TRUE ORDER BY sort_order ASC"
            )
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default()
        }
        _ => {
            sqlx::query_as(
                "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
                 FROM subscription_plans ORDER BY sort_order ASC"
            )
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default()
        }
    };
    Json(json!({ "success": true, "data": plans }))
}

/// 商户端：获取已启用的套餐列表（无需管理员权限）
async fn list_enabled_plans(
    State(state): State<AppState>,
) -> Json<Value> {
    let plans: Vec<SubscriptionPlan> = sqlx::query_as(
        "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
         FROM subscription_plans WHERE enabled = TRUE ORDER BY sort_order ASC"
    )
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
    Json(json!({ "success": true, "data": plans }))
}

async fn create_plan(
    State(state): State<AppState>,
    Json(body): Json<CreatePlanRequest>,
) -> Json<Value> {
    if body.plan != "free" && body.plan != "pro" {
        return Json(json!({"success": false, "message": "plan 只能是 free 或 pro"}));
    }
    if body.name.is_empty() {
        return Json(json!({"success": false, "message": "名称不能为空"}));
    }
    if body.price < 0.0 {
        return Json(json!({"success": false, "message": "价格不能为负数"}));
    }

    let result = sqlx::query_as::<_, SubscriptionPlan>(
        r#"
        INSERT INTO subscription_plans (plan, name, days, price, original_price, badge, highlight, sort_order)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at
        "#,
    )
    .bind(&body.plan)
    .bind(&body.name)
    .bind(body.days)
    .bind(body.price)
    .bind(body.original_price)
    .bind(&body.badge)
    .bind(body.highlight.unwrap_or(false))
    .bind(body.sort_order.unwrap_or(0))
    .fetch_optional(&state.pool)
    .await;

    match result {
        Ok(Some(p)) => Json(json!({ "success": true, "data": p })),
        Ok(None) => Json(json!({"success": false, "message": "创建失败"})),
        Err(e) => {
            if e.to_string().contains("duplicate key") {
                Json(json!({"success": false, "message": "plan 标识已存在"}))
            } else {
                Json(json!({"success": false, "message": format!("创建失败: {}", e)}))
            }
        }
    }
}

async fn update_plan(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePlanRequest>,
) -> Json<Value> {
    let result = sqlx::query_as::<_, SubscriptionPlan>(
        r#"
        UPDATE subscription_plans SET
            name           = COALESCE($1, name),
            days           = COALESCE($2, days),
            price          = COALESCE($3, price),
            original_price = COALESCE($4, original_price),
            badge          = $5,
            highlight      = COALESCE($6, highlight),
            sort_order     = COALESCE($7, sort_order),
            enabled        = COALESCE($8, enabled),
            updated_at     = NOW()
        WHERE id = $9
        RETURNING id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at
        "#,
    )
    .bind(&body.name)
    .bind(body.days)
    .bind(body.price)
    .bind(body.original_price)
    .bind(&body.badge)
    .bind(body.highlight)
    .bind(body.sort_order)
    .bind(body.enabled)
    .bind(id)
    .fetch_optional(&state.pool)
    .await;

    match result {
        Ok(Some(p)) => Json(json!({ "success": true, "data": p })),
        Ok(None) => Json(json!({"success": false, "message": "套餐不存在"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

async fn delete_plan(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let result = sqlx::query("DELETE FROM subscription_plans WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({ "success": true })),
        Ok(_) => Json(json!({"success": false, "message": "套餐不存在"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

/// 供 payments.rs 内部调用：查询所有已启用的套餐
pub async fn get_enabled_plans(pool: &sqlx::PgPool) -> Vec<SubscriptionPlan> {
    sqlx::query_as(
        "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
         FROM subscription_plans WHERE enabled = TRUE ORDER BY sort_order ASC"
    )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}
