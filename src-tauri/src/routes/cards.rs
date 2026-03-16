use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{AppState, auth_middleware},
    models::card::Card,
    routes::plan_config::get_config_by_plan,
    utils::{card_gen::generate_card_code, jwt::Claims},
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
pub struct GenerateCardsRequest {
    pub app_id: Uuid,
    pub count: u32,
    pub duration_days: i32,
    pub max_devices: i32,
    pub note: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct CardQuery {
    pub app_id: Option<Uuid>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Deserialize)]
pub struct BatchCardStatusRequest {
    pub ids: Vec<Uuid>,
    /// "disabled" 或 "unused"（启用）
    pub action: String,
}

pub fn cards_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/cards", get(list_cards).post(generate_cards))
        .route("/cards/batch-status", post(batch_update_card_status))
        .route("/cards/:id", get(get_card).delete(delete_card))
        .route("/cards/:id/disable", patch(disable_card))
        .route("/cards/:id/enable", patch(enable_card))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<CardQuery>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let (mut cards, total): (Vec<Card>, i64) = if claims.role == "admin" {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cards")
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));
        let cards: Vec<Card> = sqlx::query_as(
            "SELECT * FROM cards ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (cards, total.0)
    } else {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM cards WHERE merchant_id = $1",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let cards: Vec<Card> = sqlx::query_as(
            "SELECT * FROM cards WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(merchant_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (cards, total.0)
    };

    // 解密所有卡密代码
    for card in &mut cards {
        if let Err(e) = EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &card.code) {
            tracing::warn!("解密卡密 {} 失败: {}", card.id, e);
        }
    }

    Json(json!({
        "success": true,
        "data": cards,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

async fn generate_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<GenerateCardsRequest>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    if body.count == 0 || body.count > 1000 {
        return Json(json!({"success": false, "message": "生成数量需在 1-1000 之间"}));
    }
    if body.duration_days <= 0 {
        return Json(json!({"success": false, "message": "有效天数必须大于0"}));
    }
    if body.max_devices <= 0 || body.max_devices > 100 {
        return Json(json!({"success": false, "message": "设备数量需在 1-100 之间"}));
    }

    // 非管理员检查套餐限制
    if claims.role != "admin" {
        let plan: (String,) = sqlx::query_as("SELECT plan FROM merchants WHERE id = $1")
            .bind(merchant_id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or_else(|_| ("free".to_string(),));
        let config = get_config_by_plan(&state.pool, &plan.0).await;

        if config.max_gen_once != -1 && body.count > config.max_gen_once as u32 {
            return Json(json!({
                "success": false,
                "message": format!("{}单次最多生成 {} 张卡密", config.label, config.max_gen_once)
            }));
        }
        if config.max_cards != -1 {
            let card_count: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM cards WHERE merchant_id = $1")
                    .bind(merchant_id)
                    .fetch_one(&state.pool)
                    .await
                    .unwrap_or((0,));
            if card_count.0 + body.count as i64 > config.max_cards as i64 {
                return Json(json!({
                    "success": false,
                    "message": format!("{}最多拥有 {} 张卡密（当前已有 {} 张），请升级套餐", config.label, config.max_cards, card_count.0)
                }));
            }
        }
        if config.max_devices != -1 && body.max_devices > config.max_devices {
            return Json(json!({
                "success": false,
                "message": format!("{}单张卡密最多绑定 {} 台设备，请升级套餐", config.label, config.max_devices)
            }));
        }
    }

    // 验证 app 归属
    let app_exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM apps WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin') AND status = 'active'",
    )
    .bind(body.app_id)
    .bind(merchant_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if app_exists.is_none() {
        return Json(json!({"success": false, "message": "应用不存在或已禁用"}));
    }

    // 批量生成：先在内存中生成所有 code 并加密
    let codes: Vec<String> = (0..body.count).map(|_| generate_card_code()).collect();
    
    // 加密所有卡密代码并生成哈希
    let mut encrypted_codes = Vec::new();
    for code in &codes {
        let temp_id = Uuid::new_v4();
        let key_id = format!("card_code_{}", temp_id);
        let code_hash = EncryptedFieldsOps::generate_hash(code);
        match state.encryptor.encrypt(code, &key_id) {
            Ok(encrypted) => encrypted_codes.push((temp_id, encrypted, code_hash)),
            Err(e) => return Json(json!({"success": false, "message": format!("加密失败: {}", e)})),
        }
    }

    // 构建 VALUES 占位符：($1,$2,$3,$4,$5,$6,$7), ($8,...) ...
    let mut params_sql = String::new();
    let base = 7usize;
    for i in 0..encrypted_codes.len() {
        let n = i * base;
        if i > 0 { params_sql.push(','); }
        params_sql.push_str(&format!(
            "(${},${},${},${},${},${},${})",
            n+1, n+2, n+3, n+4, n+5, n+6, n+7
        ));
    }
    let sql = format!(
        "INSERT INTO cards (app_id, merchant_id, code_encrypted, code_hash, duration_days, max_devices, note) VALUES {} RETURNING id",
        params_sql
    );

    let mut q = sqlx::query_as::<_, (Uuid,)>(&sql);
    for (_, encrypted_code, code_hash) in &encrypted_codes {
        q = q
            .bind(body.app_id)
            .bind(merchant_id)
            .bind(encrypted_code)
            .bind(code_hash)
            .bind(body.duration_days)
            .bind(body.max_devices)
            .bind(&body.note);
    }
    
    match q.fetch_all(&state.pool).await {
        Ok(inserted_cards) => {
            // 记录加密日志
            let pool = state.pool.clone();
            let encrypted_codes_clone = encrypted_codes.clone();
            tokio::spawn(async move {
                for ((temp_id, _, _), (card_id,)) in encrypted_codes_clone.iter().zip(inserted_cards.iter()) {
                    let key_id = format!("card_code_{}", temp_id);
                    if let Err(e) = EncryptedFieldsOps::log_encryption(
                        &pool,
                        "cards",
                        *card_id,
                        "code",
                        &key_id,
                    ).await {
                        tracing::error!("记录加密日志失败: {}", e);
                    }
                }
            });

            Json(json!({
                "success": true,
                "message": format!("成功生成 {} 张卡密", encrypted_codes.len()),
                "count": encrypted_codes.len()
            }))
        }
        Err(e) => Json(json!({"success": false, "message": format!("生成失败: {}", e)})),
    }
}

async fn get_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let mut card: Option<Card> = sqlx::query_as(
        "SELECT * FROM cards WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin')",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if let Some(ref mut c) = card {
        if let Err(e) = EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &c.code) {
            tracing::warn!("解密卡密 {} 失败: {}", c.id, e);
        }
    }

    match card {
        Some(c) => Json(json!({"success": true, "data": c})),
        None => Json(json!({"success": false, "message": "卡密不存在"})),
    }
}

async fn delete_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = sqlx::query(
        "DELETE FROM cards WHERE id = $1 AND (merchant_id = $2 OR $3 = 'admin') AND status = 'unused'",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(&claims.role)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "删除成功"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、已使用或无权限"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

async fn disable_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = if claims.role == "admin" {
        // 管理员禁用：打 admin_disabled 标记
        sqlx::query(
            "UPDATE cards SET status = 'disabled', admin_disabled = TRUE WHERE id = $1",
        )
        .bind(id)
        .execute(&state.pool)
        .await
    } else {
        sqlx::query(
            "UPDATE cards SET status = 'disabled' WHERE id = $1 AND merchant_id = $2 AND admin_disabled = FALSE",
        )
        .bind(id)
        .bind(merchant_id)
        .execute(&state.pool)
        .await
    };

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "卡密已禁用"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、无权限或已被管理员锁定"})),
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
    }
}

async fn enable_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();
    let result = if claims.role == "admin" {
        // 管理员启用：同时清除 admin_disabled
        sqlx::query(
            "UPDATE cards SET status = 'unused', admin_disabled = FALSE WHERE id = $1 AND status = 'disabled'",
        )
        .bind(id)
        .execute(&state.pool)
        .await
    } else {
        // 商户：只能启用非管理员禁用的卡密
        sqlx::query(
            "UPDATE cards SET status = 'unused' WHERE id = $1 AND status = 'disabled' AND merchant_id = $2 AND admin_disabled = FALSE",
        )
        .bind(id)
        .bind(merchant_id)
        .execute(&state.pool)
        .await
    };

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "卡密已启用"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、状态不符、无权限或已被管理员锁定"})),
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
    }
}

/// 批量禁用/启用卡密（单条 SQL ANY，防止大量请求冲击数据库）
async fn batch_update_card_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<BatchCardStatusRequest>,
) -> Json<Value> {
    if body.ids.is_empty() {
        return Json(json!({"success": false, "message": "ids 不能为空"}));
    }
    if body.ids.len() > 500 {
        return Json(json!({"success": false, "message": "单次批量操作最多 500 张"}));
    }
    let merchant_id = Uuid::parse_str(&claims.sub).unwrap_or_default();

    let result = match body.action.as_str() {
        "disabled" => {
            if claims.role == "admin" {
                sqlx::query(
                    "UPDATE cards SET status = 'disabled', admin_disabled = TRUE WHERE id = ANY($1)",
                )
                .bind(&body.ids)
                .execute(&state.pool)
                .await
            } else {
                sqlx::query(
                    "UPDATE cards SET status = 'disabled' WHERE id = ANY($1) AND merchant_id = $2 AND admin_disabled = FALSE",
                )
                .bind(&body.ids)
                .bind(merchant_id)
                .execute(&state.pool)
                .await
            }
        }
        "unused" => {
            if claims.role == "admin" {
                sqlx::query(
                    "UPDATE cards SET status = 'unused', admin_disabled = FALSE WHERE id = ANY($1) AND status = 'disabled'",
                )
                .bind(&body.ids)
                .execute(&state.pool)
                .await
            } else {
                // 商户：排除 admin_disabled 的卡密
                sqlx::query(
                    "UPDATE cards SET status = 'unused' WHERE id = ANY($1) AND status = 'disabled' AND merchant_id = $2 AND admin_disabled = FALSE",
                )
                .bind(&body.ids)
                .bind(merchant_id)
                .execute(&state.pool)
                .await
            }
        }
        _ => return Json(json!({"success": false, "message": "action 仅支持 disabled / unused"})),
    };

    match result {
        Ok(r) => Json(json!({
            "success": true,
            "message": format!("已更新 {} 张卡密", r.rows_affected())
        })),
        Err(e) => Json(json!({"success": false, "message": format!("批量操作失败: {}", e)})),
    }
}
