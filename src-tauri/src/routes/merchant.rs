use crate::{
    middleware::auth::{AppState, auth_middleware},
    utils::jwt::Claims,
};
use axum::{
    extract::{Query, State},
    middleware,
    routing::{get, post},
    Extension, Json, Router,
};
use bcrypt::{hash, DEFAULT_COST};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Deserialize)]
pub struct DashboardQuery {
    pub range: Option<String>,
}

pub fn merchant_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/merchant/profile", get(get_profile))
        .route("/merchant/dashboard-stats", get(dashboard_stats))
        .route("/merchant/change-password", post(change_password))
        .route("/merchant/regenerate-apikey", post(regenerate_api_key))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn get_profile(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    let merchant: Option<crate::models::merchant::Merchant> =
        sqlx::query_as("SELECT * FROM merchants WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    match merchant {
        Some(m) => {
            let public: crate::models::merchant::MerchantPublic = m.into();
            Json(json!({"success": true, "data": public}))
        }
        None => Json(json!({"success": false, "message": "用户不存在"})),
    }
}

async fn dashboard_stats(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<DashboardQuery>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    // 根据 range 参数决定时间区间和分组粒度
    let (interval, trunc, label) = match q.range.as_deref().unwrap_or("week") {
        "month" => ("3 months", "week", "month"),
        "year"  => ("1 year",   "month", "year"),
        _       => ("7 days",   "day",   "week"),  // 默认周
    };
    let _ = label;

    // 1. 卡密使用率
    let card_stats: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM cards WHERE merchant_id = $1 GROUP BY status",
    )
    .bind(merchant_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // 2. 激活趋势（动态粒度）
    let sql = format!(
        "SELECT DATE_TRUNC('{trunc}', activated_at)::date AS day, COUNT(*) AS cnt
         FROM activations
         WHERE card_id IN (SELECT id FROM cards WHERE merchant_id = $1)
           AND activated_at >= NOW() - INTERVAL '{interval}'
         GROUP BY day
         ORDER BY day",
        trunc = trunc,
        interval = interval,
    );
    let activation_trend: Vec<(chrono::NaiveDate, i64)> =
        sqlx::query_as(&sql)
            .bind(merchant_id)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

    // 3. 设备分布
    let device_dist: Vec<(String, i64)> = sqlx::query_as(
        "SELECT a.app_name, COUNT(act.id) AS device_cnt
         FROM apps a
         LEFT JOIN cards c ON c.app_id = a.id AND c.merchant_id = $1
         LEFT JOIN activations act ON act.card_id = c.id
         WHERE a.merchant_id = $1
         GROUP BY a.app_name
         ORDER BY device_cnt DESC
         LIMIT 10",
    )
    .bind(merchant_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(json!({
        "success": true,
        "data": {
            "card_stats": card_stats.iter().map(|(s, c)| json!({"status": s, "count": c})).collect::<Vec<_>>(),
            "activation_trend": activation_trend.iter().map(|(d, c)| json!({"date": d.to_string(), "count": c})).collect::<Vec<_>>(),
            "device_dist": device_dist.iter().map(|(app, c)| json!({"app": app, "count": c})).collect::<Vec<_>>(),
        }
    }))
}

async fn change_password(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<ChangePasswordRequest>,
) -> Json<Value> {
    if body.new_password.len() < 8 {
        return Json(json!({"success": false, "message": "新密码至少8位"}));
    }
    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    let merchant: Option<crate::models::merchant::Merchant> =
        sqlx::query_as("SELECT * FROM merchants WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant = match merchant {
        Some(m) => m,
        None => return Json(json!({"success": false, "message": "用户不存在"})),
    };

    let valid = bcrypt::verify(&body.old_password, &merchant.password_hash).unwrap_or(false);
    if !valid {
        return Json(json!({"success": false, "message": "原密码错误"}));
    }

    let new_hash = match hash(&body.new_password, DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    let _ = sqlx::query(
        "UPDATE merchants SET password_hash = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&new_hash)
    .bind(id)
    .execute(&state.pool)
    .await;

    Json(json!({"success": true, "message": "密码已修改"}))
}

async fn regenerate_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    let new_key = crate::utils::card_gen::generate_api_key();
    let _ = sqlx::query(
        "UPDATE merchants SET api_key = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&new_key)
    .bind(id)
    .execute(&state.pool)
    .await;

    Json(json!({"success": true, "data": {"api_key": new_key}}))
}

