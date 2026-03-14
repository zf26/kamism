use crate::{
    middleware::auth::{AppState, auth_middleware},
    utils::jwt::Claims,
};
use axum::{
    extract::State,
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

pub fn merchant_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/merchant/profile", get(get_profile))
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

