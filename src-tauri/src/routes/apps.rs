use crate::{
    middleware::auth::{AppState, auth_middleware},
    models::app::App,
    routes::plan_config::get_config_by_plan,
    utils::{db_guard, jwt::Claims},
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{get, patch, post},
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

#[derive(Deserialize)]
pub struct BatchStatusRequest {
    pub ids: Vec<Uuid>,
    pub status: String,
}

pub fn apps_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/apps", get(list_apps).post(create_app))
        .route("/apps/batch-status", post(batch_update_app_status))
        .route("/apps/:id", get(get_app).delete(delete_app))
        .route("/apps/:id/status", patch(update_app_status))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

fn merchant_id_from_claims(claims: &Claims) -> Result<Uuid, Json<Value>> {
    Uuid::parse_str(&claims.sub)
        .map_err(|_| Json(json!({"success": false, "message": "无效用户ID"})))
}

async fn list_apps(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<AppQuery>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // ⚠️ 曾用 `.unwrap_or((0,))`：列表页的 total 静默变 0。
    // 注意它和下面的 `apps` 列表是**两个查询**：如果只有 total 失败，
    // 就会出现「列表有 5 条，但总数显示 0」这种自相矛盾的页面。
    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM apps WHERE merchant_id = $1")
            .bind(merchant_id)
            .fetch_one(&state.pool),
        "统计应用总数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(t) => t,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    // ⚠️ 曾用 .unwrap_or_default()：数据库故障时应用列表变空，
    // 商户看到「还没有创建任何应用」并可能立刻去重新创建应用，
    // 而上面刚查出来的 total 明明非 0 —— 一个自相矛盾的页面。
    let apps: Vec<App> = match sqlx::query_as(
        "SELECT * FROM apps WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(merchant_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询应用列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    Json(json!({
        "success": true,
        "data": apps,
        "total": total.0,
        "page": page,
        "page_size": page_size
    }))
}

async fn create_app(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateAppRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    if body.app_name.trim().is_empty() {
        return Json(json!({"success": false, "message": "应用名称不能为空"}));
    }

    // 检查套餐限制
    //
    // 查询失败时按免费版处理，方向是安全的（配额更严，不会放宽），这个行为保留；
    // 但**必须留痕** —— 否则商户看到的是「免费版最多创建 N 个应用，请升级套餐」
    // 这句业务话术，而真相是数据库故障，排查方向被彻底带偏。
    // （`subscription_plan.rs::get_enabled_plans` 的注释里记过同一类坑。）
    let plan: (String,) = match sqlx::query_as("SELECT plan FROM merchants WHERE id = $1")
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "查询商户套餐失败，本次配额检查按免费版处理: merchant_id={} err={}",
                merchant_id,
                e
            );
            ("free".to_string(),)
        }
    };

    let config = get_config_by_plan(&state.pool, &plan.0).await;

    // ⚠️⚠️ 这是**配额上限检查**，`.unwrap_or((0,))` 的方向是**放松**：
    // 查询失败 → 已创建数当成 0 → `0 >= max_apps` 为假 → **跳过配额检查** →
    // 继续创建应用。
    // 也就是说：数据库一抖，免费用户就能无限建应用（免费套餐 `max_apps = 1`）。
    // 而且这个窗口期的创建**不会留下任何异常日志** —— 事后统计时
    // 只会看到「有几个用户的套餐数和实际应用数对不上」。
    //
    // 配额检查属于必须 fail-closed 的判定：算不出已用量，就不能放行。
    let app_count = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM apps WHERE merchant_id = $1")
            .bind(merchant_id)
            .fetch_one(&state.pool),
        "统计已有应用数（配额检查）",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(c) => c,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    if config.max_apps != -1 && app_count.0 >= config.max_apps as i64 {
        return Json(json!({
            "success": false,
            "message": format!("{}最多创建 {} 个应用，请升级套餐", config.label, config.max_apps)
        }));
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
        Err(e) => db_guard::internal_error("创建应用", e),
    }
}

async fn get_app(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「应用不存在或无权限」。
    // 用户正看着这个应用，却说它不存在。
    let app = match db_guard::optional(
        sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = $1 AND merchant_id = $2")
            .bind(id)
            .bind(merchant_id)
            .fetch_optional(&state.pool),
        "查询应用详情",
    )
    .await
    {
        db_guard::QueryOutcome::Found(a) => Some(a),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    match app {
        Some(a) => Json(json!({"success": true, "data": a})),
        None => Json(json!({"success": false, "message": "应用不存在或无权限"})),
    }
}

async fn delete_app(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = sqlx::query(
        "DELETE FROM apps WHERE id = $1 AND merchant_id = $2",
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "删除成功"})),
        Ok(_) => Json(json!({"success": false, "message": "应用不存在或无权限"})),
        Err(e) => db_guard::internal_error("删除应用", e),
    }
}

async fn update_app_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let status = match body.get("status").and_then(|s| s.as_str()) {
        Some(s) if s == "active" || s == "disabled" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效状态"})),
    };

    // 商户操作：不允许启用被管理员禁用的应用
    if status == "active" {
        // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 落到 `None` 分支 → 拒绝启用。
        // 这里**方向是安全的**（fail-closed，不会误放行管理员禁用的应用），
        // 但文案「应用不存在或无权限」是假结论：应用明明就在列表里。
        // 拆开之后，管理员才能从日志区分「权限问题」和「数据库问题」。
        let app = match db_guard::optional(
            sqlx::query_as::<_, (bool,)>("SELECT admin_disabled FROM apps WHERE id = $1 AND merchant_id = $2")
                .bind(id)
                .bind(merchant_id)
                .fetch_optional(&state.pool),
            "查询应用是否被管理员禁用（启用前校验）",
        )
        .await
        {
            db_guard::QueryOutcome::Found(v) => Some(v),
            db_guard::QueryOutcome::NotFound => None,
            db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
        };
        match app {
            Some((true,)) => return Json(json!({"success": false, "message": "该应用已被管理员禁用，无法自行启用"})),
            None => return Json(json!({"success": false, "message": "应用不存在或无权限"})),
            _ => {}
        }
    }

    let result = sqlx::query(
        "UPDATE apps SET status = $1, updated_at = NOW() WHERE id = $2 AND merchant_id = $3",
    )
    .bind(&status)
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "状态已更新"})),
        Ok(_) => Json(json!({"success": false, "message": "应用不存在或无权限"})),
        Err(e) => db_guard::internal_error("更新应用", e),
    }
}

async fn batch_update_app_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<BatchStatusRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if body.ids.is_empty() {
        return Json(json!({"success": false, "message": "ids 不能为空"}));
    }
    if body.ids.len() > 200 {
        return Json(json!({"success": false, "message": "单次批量操作最多 200 条"}));
    }
    let status = match body.status.as_str() {
        s if s == "active" || s == "disabled" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效状态"})),
    };

    let result = if status == "active" {
        // 商户批量启用：排除被管理员禁用的应用
        sqlx::query(
            "UPDATE apps SET status = $1, updated_at = NOW()
             WHERE id = ANY($2) AND merchant_id = $3 AND admin_disabled = FALSE",
        )
        .bind(&status)
        .bind(&body.ids)
        .bind(merchant_id)
        .execute(&state.pool)
        .await
    } else {
        sqlx::query(
            "UPDATE apps SET status = $1, updated_at = NOW()
             WHERE id = ANY($2) AND merchant_id = $3",
        )
        .bind(&status)
        .bind(&body.ids)
        .bind(merchant_id)
        .execute(&state.pool)
        .await
    };

    match result {
        Ok(r) => Json(json!({
            "success": true,
            "message": format!("已更新 {} 个应用", r.rows_affected())
        })),
        Err(e) => db_guard::internal_error("批量更新应用", e),
    }
}
