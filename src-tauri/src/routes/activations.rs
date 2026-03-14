use crate::{
    middleware::auth::{AppState, auth_middleware},
    models::activation::Activation,
    utils::jwt::Claims,
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct ActivationQuery {
    pub card_id: Option<Uuid>,
    pub app_id: Option<Uuid>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub fn activations_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/activations", get(list_activations))
        .route("/activations/:id", delete(unbind_device))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_activations(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<ActivationQuery>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let activations: Vec<Activation> = if claims.role == "admin" {
        sqlx::query_as(
            "SELECT * FROM activations ORDER BY activated_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as(
            r#"SELECT a.* FROM activations a
               JOIN cards c ON c.id = a.card_id
               WHERE c.merchant_id = $1
               ORDER BY a.activated_at DESC
               LIMIT $2 OFFSET $3"#,
        )
        .bind(merchant_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
    };

    let total: (i64,) = if claims.role == "admin" {
        sqlx::query_as("SELECT COUNT(*) FROM activations")
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,))
    } else {
        sqlx::query_as(
            "SELECT COUNT(*) FROM activations a JOIN cards c ON c.id = a.card_id WHERE c.merchant_id = $1",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,))
    };

    Json(json!({
        "success": true,
        "data": activations,
        "total": total.0,
        "page": page,
        "page_size": page_size
    }))
}

async fn unbind_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();

    // 解绑设备后，检查该卡密是否还有其他激活记录，若无则恢复为 unused
    let activation: Option<Activation> = sqlx::query_as(
        r#"SELECT a.* FROM activations a
           JOIN cards c ON c.id = a.card_id
           WHERE a.id = $1 AND (c.merchant_id = $2 OR $3 = 'admin')"#,
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let activation = match activation {
        Some(a) => a,
        None => return Json(json!({"success": false, "message": "记录不存在或无权限"})),
    };

    let _ = sqlx::query("DELETE FROM activations WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await;

    // 检查该卡密剩余激活数
    let remaining: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM activations WHERE card_id = $1")
            .bind(activation.card_id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));

    if remaining.0 == 0 {
        let _ = sqlx::query(
            "UPDATE cards SET status = 'unused', activated_at = NULL, expires_at = NULL WHERE id = $1 AND status = 'active'",
        )
        .bind(activation.card_id)
        .execute(&state.pool)
        .await;
    }

    Json(json!({"success": true, "message": "设备已解绑"}))
}

