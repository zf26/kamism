use crate::middleware::auth::{admin_only, auth_middleware, AppState};
use crate::models::plan_config::PlanConfig;
use axum::{
    extract::{Path, State},
    middleware,
    routing::{get, patch},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct UpdatePlanConfigRequest {
    pub label: Option<String>,
    pub max_apps: Option<i32>,
    pub max_cards: Option<i32>,
    pub max_devices: Option<i32>,
    pub max_gen_once: Option<i32>,
}

pub fn plan_config_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/plan-configs", get(list_plan_configs))
        .route("/admin/plan-configs/:id", patch(update_plan_config))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_plan_configs(State(state): State<AppState>) -> Json<Value> {
    let configs: Vec<PlanConfig> =
        sqlx::query_as("SELECT * FROM plan_configs ORDER BY plan ASC")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();
    Json(json!({ "success": true, "data": configs }))
}

async fn update_plan_config(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePlanConfigRequest>,
) -> Json<Value> {
    // 校验 max_devices 不能超过 100
    if let Some(d) = body.max_devices {
        if d != -1 && (d < 1 || d > 100) {
            return Json(json!({"success": false, "message": "max_devices 需在 1-100 之间（-1 表示无限）"}));
        }
    }

    let result = sqlx::query(
        "UPDATE plan_configs SET
            label        = COALESCE($1, label),
            max_apps     = COALESCE($2, max_apps),
            max_cards    = COALESCE($3, max_cards),
            max_devices  = COALESCE($4, max_devices),
            max_gen_once = COALESCE($5, max_gen_once),
            updated_at   = NOW()
         WHERE id = $6",
    )
    .bind(&body.label)
    .bind(body.max_apps)
    .bind(body.max_cards)
    .bind(body.max_devices)
    .bind(body.max_gen_once)
    .bind(id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let updated: Option<PlanConfig> =
                sqlx::query_as("SELECT * FROM plan_configs WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&state.pool)
                    .await
                    .unwrap_or(None);
            Json(json!({ "success": true, "data": updated }))
        }
        Ok(_) => Json(json!({"success": false, "message": "套餐配置不存在"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

/// 供业务接口内部调用：根据 plan 名称查询配置
pub async fn get_config_by_plan(
    pool: &sqlx::PgPool,
    plan: &str,
) -> PlanConfig {
    match sqlx::query_as("SELECT * FROM plan_configs WHERE plan = $1")
        .bind(plan)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            tracing::warn!("套餐配置不存在 (plan={})，使用默认限制", plan);
            default_plan_config(plan)
        }
        Err(e) => {
            tracing::error!("查询套餐配置失败 (plan={}): {}，使用默认限制", plan, e);
            default_plan_config(plan)
        }
    }
}

fn default_plan_config(plan: &str) -> PlanConfig {
    PlanConfig {
        id: Uuid::nil(),
        plan: plan.to_string(),
        label: if plan == "pro" { "专业版".to_string() } else { "免费版".to_string() },
        max_apps: if plan == "pro" { -1 } else { 1 },
        max_cards: if plan == "pro" { -1 } else { 500 },
        max_devices: if plan == "pro" { 100 } else { 3 },
        max_gen_once: if plan == "pro" { 1000 } else { 100 },
        updated_at: chrono::Utc::now(),
    }
}

