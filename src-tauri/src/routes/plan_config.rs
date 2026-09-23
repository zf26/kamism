use crate::middleware::auth::{admin_only, auth_middleware, AppState};
use crate::models::plan_config::PlanConfig;
use crate::utils::db_guard;
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
    // ⚠️ 曾用 .unwrap_or_default()：查询失败时套餐配置列表变空，
    // 管理员在「套餐配置」页看到一片空白，会以为套餐配置被删光了并去重建
    // （而重建会覆盖真实配置）—— 实际只是数据库查不出来。
    let configs: Vec<PlanConfig> = match sqlx::query_as("SELECT * FROM plan_configs ORDER BY plan ASC")
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询套餐配置列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };
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
            // ⚠️ 曾用 `.unwrap_or(None)`：更新**已经成功**，但回读失败 →
            // 返回 `data: null`。前端拿到 `success: true` 却没有任何数据，
            // 常见的处理是「把表单清空」或「显示空配置」——
            // 管理员会以为自己刚保存的配额被重置了，然后**再保存一次**。
            //
            // 注意这里不能因为回读失败就返回失败：UPDATE 确实成功了，
            // 说「更新失败」会让管理员重复操作。改成如实返回「已保存，但回读失败」。
            let updated: Option<PlanConfig> = match sqlx::query_as(
                "SELECT * FROM plan_configs WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.pool)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("套餐配置已更新但回读失败: id={} err={}", id, e);
                    return Json(json!({
                        "success": true,
                        "message": "配置已保存，但读取最新数据失败，请刷新页面确认",
                        "data": null
                    }));
                }
            };
            Json(json!({ "success": true, "data": updated }))
        }
        Ok(_) => Json(json!({"success": false, "message": "套餐配置不存在"})),
        Err(e) => db_guard::internal_error("更新套餐配置", e),
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

