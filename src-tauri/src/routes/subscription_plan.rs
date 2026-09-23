use crate::middleware::auth::{admin_only, auth_middleware, AppState};
use crate::models::subscription_plan::{CreatePlanRequest, SubscriptionPlan, UpdatePlanRequest};
use crate::utils::db_guard;
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
            // ⚠️ 曾用 .unwrap_or_default()：查询失败时只过滤了启用项的套餐列表变空，
            // 管理员看到「暂无套餐」，会以为套餐被删光了并去重新创建。
            match sqlx::query_as(
                "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
                 FROM subscription_plans WHERE enabled = TRUE ORDER BY sort_order ASC"
            )
                .fetch_all(&state.pool)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("查询已启用套餐列表失败: err={}", e);
                    return db_guard::server_busy();
                }
            }
        }
        _ => {
            // ⚠️ 曾用 .unwrap_or_default()：同上，管理端套餐列表静默变空。
            match sqlx::query_as(
                "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
                 FROM subscription_plans ORDER BY sort_order ASC"
            )
                .fetch_all(&state.pool)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("查询套餐列表失败: err={}", e);
                    return db_guard::server_busy();
                }
            }
        }
    };
    Json(json!({ "success": true, "data": plans }))
}

/// 商户端：获取已启用的套餐列表（无需管理员权限）
async fn list_enabled_plans(
    State(state): State<AppState>,
) -> Json<Value> {
    // ⚠️ 曾用 .unwrap_or_default()：商户端购买页套餐列表查询失败时变空，
    // 商户看到「暂无可购买套餐」，会以为平台停售了，无法下单；
    // 运维只看到「没人买」，看不到源头是数据库查询失败。
    let plans: Vec<SubscriptionPlan> = match sqlx::query_as(
        "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
         FROM subscription_plans WHERE enabled = TRUE ORDER BY sort_order ASC"
    )
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询可购买套餐列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };
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
            // ⚠️ 唯一冲突必须用 **SQLSTATE** 判，不能用错误文本。
            //
            // 原先这里是 `e.to_string().contains("duplicate key")` —— 而 sqlx 的
            // Display 是数据库自己那句话，随 `lc_messages` 变化。本机 PG（中文）
            // 实测报「重复键违反唯一约束"uq_subscription_plans_plan_days"」，
            // 于是那个判断**永远不成立**，管理员建重复套餐看到的是
            // 「服务器繁忙，请稍后重试」：一个输入错误被报成了服务器故障。
            //
            // 文案也一并改准：唯一约束是 `UNIQUE (plan, days)`（007 迁移把
            // 原来的 `UNIQUE(plan)` 换掉了），所以冲突的粒度是「组合」，
            // 不是「plan 标识」——原文案即使能命中也是错的。
            if db_guard::is_unique_violation(&e) {
                Json(json!({
                    "success": false,
                    "message": "该套餐已存在（同一个 plan 下的 days 不能重复）"
                }))
            } else {
                db_guard::internal_error("创建套餐", e)
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
        Err(e) => db_guard::internal_error("更新套餐", e),
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
        Err(e) => db_guard::internal_error("删除套餐", e),
    }
}

/// 供 payments.rs 内部调用：查询所有已启用的套餐
///
/// ⚠️ 返回类型是 `Vec` 而非 `Result`，签名由 payments.rs 决定，这里不能单方面改成
/// 报错。曾用 `.unwrap_or_default()`（无日志）：上游下单流程拿到空列表后会走到
/// 「套餐不存在」分支，商户看到的是「套餐已下架」这种业务话术，
/// 而真相是数据库故障 —— 排查方向被彻底带偏。故降级留痕（warn 日志）。
pub async fn get_enabled_plans(pool: &sqlx::PgPool) -> Vec<SubscriptionPlan> {
    db_guard::lenient_all(
        sqlx::query_as(
            "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, badge, highlight, sort_order, enabled, created_at, updated_at \
             FROM subscription_plans WHERE enabled = TRUE ORDER BY sort_order ASC"
        )
            .fetch_all(pool),
        "查询已启用套餐（给下单流程用，失败按空列表降级）",
    )
    .await
}
