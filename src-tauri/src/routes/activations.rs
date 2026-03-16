use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{AppState, auth_middleware},
    utils::jwt::Claims,
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get},
    Extension, Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ActivationWithCode {
    pub id: Uuid,
    pub card_id: Uuid,
    pub card_code: String,
    pub app_id: Uuid,
    pub device_id: String,
    pub device_name: Option<String>,
    pub ip_address: Option<String>,
    pub activated_at: DateTime<Utc>,
    pub last_verified_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct ActivationQuery {
    pub card_code: Option<String>,
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
    let card_code_filter = q.card_code.as_deref().unwrap_or("").trim().to_lowercase();
    let has_filter = !card_code_filter.is_empty();

    // 查询激活记录（包含加密的code和device_id）
    let raw_activations: Vec<(Uuid, Uuid, String, Uuid, String, Option<String>, Option<String>, DateTime<Utc>, DateTime<Utc>)> = if claims.role == "admin" {
        sqlx::query_as(
            r#"SELECT a.id, a.card_id, c.code_encrypted, a.app_id, a.device_id_encrypted,
                      a.device_name, a.ip_address, a.activated_at, a.last_verified_at
               FROM activations a
               JOIN cards c ON c.id = a.card_id
               ORDER BY a.activated_at DESC LIMIT $1 OFFSET $2"#,
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as(
            r#"SELECT a.id, a.card_id, c.code_encrypted, a.app_id, a.device_id_encrypted,
                      a.device_name, a.ip_address, a.activated_at, a.last_verified_at
               FROM activations a
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

    // 解密所有记录
    let mut activations = Vec::new();
    for (id, card_id, encrypted_code, app_id, encrypted_device_id, device_name, ip_address, activated_at, last_verified_at) in raw_activations {
        let card_code = EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &encrypted_code)
            .unwrap_or_else(|_| "[解密失败]".to_string());
        
        let device_id = EncryptedFieldsOps::decrypt_device_id(&state.encryptor, &encrypted_device_id)
            .unwrap_or_else(|_| "[解密失败]".to_string());

        // 如果有过滤条件，检查是否匹配
        if has_filter && !card_code.to_lowercase().contains(&card_code_filter) {
            continue;
        }

        activations.push(ActivationWithCode {
            id,
            card_id,
            card_code,
            app_id,
            device_id,
            device_name,
            ip_address,
            activated_at,
            last_verified_at,
        });
    }

    // 计算总数
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

#[derive(sqlx::FromRow)]
struct ActivationCardId {
    card_id: Uuid,
}

async fn unbind_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();

    // 解绑设备后，检查该卡密是否还有其他激活记录，若无则恢复为 unused
    let activation: Option<ActivationCardId> = sqlx::query_as(
        r#"SELECT a.card_id FROM activations a
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

