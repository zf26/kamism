use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::merchant::MerchantPublic,
    utils::{card_gen::generate_api_key, mq, jwt::Claims},
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get, patch, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct MerchantQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub keyword: Option<String>,
    pub plan: Option<String>,
}

pub fn admin_router_with_state(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/merchants", get(list_merchants).post(create_merchant))
        .route("/admin/merchants/:id", delete(delete_merchant))
        .route("/admin/merchants/:id/status", patch(update_merchant_status))
        .route("/admin/merchants/:id/plan", patch(update_merchant_plan))
        .route("/admin/stats", get(get_stats))
        .route("/admin/stats/trends", get(get_trends))
        // 管理员商户功能：API Key 管理
        .route("/admin/api-key", get(get_admin_api_key))
        .route("/admin/api-key/regenerate", post(regenerate_admin_api_key))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_merchants(
    State(state): State<AppState>,
    Query(q): Query<MerchantQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;
    let keyword = q.keyword.unwrap_or_default();
    let keyword = &keyword[..keyword.len().min(100)]; // 限制搜索关键词长度
    let like = format!("%{}%", keyword);
    let plan_filter = q.plan.as_deref().unwrap_or("");

    let (total, merchants) = if plan_filter.is_empty() {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1",
        )
        .bind(&like)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let rows: Vec<crate::models::merchant::Merchant> = sqlx::query_as(
            "SELECT * FROM merchants WHERE username ILIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(&like)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (total.0, rows)
    } else {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1 AND plan = $2",
        )
        .bind(&like)
        .bind(plan_filter)
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));
        let rows: Vec<crate::models::merchant::Merchant> = sqlx::query_as(
            "SELECT * FROM merchants WHERE username ILIKE $1 AND plan = $2 ORDER BY created_at DESC LIMIT $3 OFFSET $4",
        )
        .bind(&like)
        .bind(plan_filter)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        (total.0, rows)
    };

    // 解密 email 和 api_key
    let public: Vec<MerchantPublic> = merchants.into_iter().map(|mut m| {
        if let Ok(plain) = EncryptedFieldsOps::decrypt_merchant_email(&state.encryptor, &m.email) {
            m.email = plain;
        } else {
            tracing::warn!("解密商户 {} email 失败", m.id);
        }
        if let Ok(plain) = EncryptedFieldsOps::decrypt_merchant_api_key(&state.encryptor, &m.api_key) {
            m.api_key = plain;
        } else {
            tracing::warn!("解密商户 {} api_key 失败", m.id);
        }
        m.into()
    }).collect();

    Json(json!({
        "success": true,
        "data": public,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

#[derive(Deserialize)]
pub struct CreateMerchantRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

async fn create_merchant(
    State(state): State<AppState>,
    Json(body): Json<CreateMerchantRequest>,
) -> Json<Value> {
    if body.username.trim().is_empty() {
        return Json(json!({"success": false, "message": "用户名不能为空"}));
    }
    if body.email.trim().is_empty() || !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }
    if body.password.len() < 6 {
        return Json(json!({"success": false, "message": "密码至少 6 位"}));
    }

    // 查重
    let exists: Option<(String,)> = sqlx::query_as(
        "SELECT id::text FROM merchants WHERE username = $1",
    )
    .bind(&body.username)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);
    if exists.is_some() {
        return Json(json!({"success": false, "message": "用户名已存在"}));
    }
    let exists_email: Option<(String,)> = sqlx::query_as(
        "SELECT id::text FROM merchants WHERE email_encrypted = $1",
    )
    .bind(&body.email) // 邮件加密前可先模糊匹配
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);
    if exists_email.is_some() {
        return Json(json!({"success": false, "message": "邮箱已被使用"}));
    }

    let password_hash = match bcrypt::hash(&body.password, bcrypt::DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    let merchant_id = Uuid::new_v4();

    // 加密并哈希敏感字段
    let email_key = format!("merchant_email_{}", merchant_id);
    let api_key_key = format!("merchant_apikey_{}", merchant_id);

    let encrypted_email = match state.encryptor.encrypt(&body.email, &email_key) {
        Ok(v) => v,
        Err(_) => return Json(json!({"success": false, "message": "加密邮箱失败"})),
    };
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);

    let raw_api_key = generate_api_key();
    let encrypted_api_key = match state.encryptor.encrypt(&raw_api_key, &api_key_key) {
        Ok(v) => v,
        Err(_) => return Json(json!({"success": false, "message": "生成 API Key 失败"})),
    };
    let api_key_hash = EncryptedFieldsOps::generate_hash(&raw_api_key);

    let result = sqlx::query(
        "INSERT INTO merchants
           (id, username, email_encrypted, email_hash, password_hash,
            api_key_encrypted, api_key_hash, email_verified, created_by_admin)
         VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, TRUE)",
    )
    .bind(merchant_id)
    .bind(&body.username)
    .bind(&encrypted_email)
    .bind(&email_hash)
    .bind(&password_hash)
    .bind(&encrypted_api_key)
    .bind(&api_key_hash)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => {
            Json(json!({
                "success": true,
                "message": "商户创建成功",
                "data": {
                    "id": merchant_id.to_string(),
                    "username": body.username,
                    "email": body.email,
                    "api_key": raw_api_key,
                }
            }))
        }
        Err(e) => Json(json!({"success": false, "message": format!("创建失败: {}", e)})),
    }
}

async fn delete_merchant(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    // 仅允许删除管理员创建的商户
    let target: Option<(bool,)> = sqlx::query_as(
        "SELECT created_by_admin FROM merchants WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let target = match target {
        Some(r) => r,
        None => return Json(json!({"success": false, "message": "商户不存在"})),
    };

    if !target.0 {
        return Json(json!({
            "success": false,
            "message": "该商户由用户自助注册，不允许删除。如需禁用请使用禁用功能。"
        }));
    }

    // apps、cards、activations、messages 等关联表均有 ON DELETE CASCADE
    // 只需删除商户主记录即可，其余自动级联清理
    let result = sqlx::query("DELETE FROM merchants WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!("管理员删除了商户 {}", id);
            Json(json!({"success": true, "message": "商户已删除，相关应用和卡密数据已一并清理"}))
        }
        Ok(_) => Json(json!({"success": false, "message": "删除失败"})),
        Err(e) => Json(json!({"success": false, "message": format!("删除失败: {}", e)})),
    }
}

async fn update_merchant_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let status = match body.get("status").and_then(|s| s.as_str()) {
        Some(s) if s == "active" || s == "disabled" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效状态"})),
    };

    let result =
        sqlx::query("UPDATE merchants SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(&status)
            .bind(id)
            .execute(&state.pool)
            .await;

    match result {
        Ok(_) => Json(json!({"success": true, "message": "状态已更新"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

async fn update_merchant_plan(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let plan = match body.get("plan").and_then(|s| s.as_str()) {
        Some(s) if s == "free" || s == "pro" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效套餐，仅支持 free / pro"})),
    };

    // expires_days: 仅 pro 有效，None 表示永久，0 表示立即到期
    let expires_days = body.get("expires_days").and_then(|v| v.as_i64());

    let result = if plan == "pro" {
        match expires_days {
            Some(days) if days > 0 => {
                sqlx::query(
                    "UPDATE merchants
                     SET plan = $1,
                         plan_expires_at = NOW() + ($2 || ' days')::INTERVAL,
                         updated_at = NOW()
                     WHERE id = $3",
                )
                .bind(&plan)
                .bind(days.to_string())
                .bind(id)
                .execute(&state.pool)
                .await
            }
            _ => {
                // 永久专业版，清空到期时间
                sqlx::query(
                    "UPDATE merchants
                     SET plan = $1,
                         plan_expires_at = NULL,
                         updated_at = NOW()
                     WHERE id = $2",
                )
                .bind(&plan)
                .bind(id)
                .execute(&state.pool)
                .await
            }
        }
    } else {
        // 手动降为免费版，清空到期时间
        sqlx::query(
            "UPDATE merchants
             SET plan = $1,
                 plan_expires_at = NULL,
                 updated_at = NOW()
             WHERE id = $2",
        )
        .bind(&plan)
        .bind(id)
        .execute(&state.pool)
        .await
    };

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let msg = if plan == "pro" {
                // 升级为专业版：异步发布恢复消息
                if let Err(e) = mq::publish_upgrade(&state.mq_channel, &id.to_string()).await {
                    tracing::error!("发布升级恢复消息失败 {}: {}", id, e);
                }
                match expires_days {
                    Some(d) if d > 0 => format!("已升级为专业版，有效期 {} 天", d),
                    _ => "已升级为专业版（永久）".to_string(),
                }
            } else {
                "已降级为免费版".to_string()
            };
            Json(json!({"success": true, "message": msg}))
        }
        Ok(_) => Json(json!({"success": false, "message": "商户不存在"})),
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
    }
}

async fn get_stats(State(state): State<AppState>) -> Json<Value> {
    let merchant_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM merchants")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    let card_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cards")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    let activation_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM activations")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    let active_card_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM cards WHERE status = 'active'")
            .fetch_one(&state.pool)
            .await
            .unwrap_or((0,));

    let app_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM apps")
        .fetch_one(&state.pool)
        .await
        .unwrap_or((0,));

    Json(json!({
        "success": true,
        "data": {
            "merchants": merchant_count.0,
            "total_cards": card_count.0,
            "active_cards": active_card_count.0,
            "total_activations": activation_count.0,
            "total_apps": app_count.0
        }
    }))
}

/// 每日增量趋势（近 30 天）
async fn get_trends(State(state): State<AppState>) -> Json<Value> {
    let merchants: Vec<(chrono::NaiveDate, i64)> = sqlx::query_as(
        r#"SELECT DATE(created_at) AS day, COUNT(*)::bigint AS cnt
           FROM merchants WHERE created_at >= NOW() - INTERVAL '30 days'
           GROUP BY day ORDER BY day"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let apps: Vec<(chrono::NaiveDate, i64)> = sqlx::query_as(
        r#"SELECT DATE(created_at) AS day, COUNT(*)::bigint AS cnt
           FROM apps WHERE created_at >= NOW() - INTERVAL '30 days'
           GROUP BY day ORDER BY day"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let cards: Vec<(chrono::NaiveDate, i64)> = sqlx::query_as(
        r#"SELECT DATE(created_at) AS day, COUNT(*)::bigint AS cnt
           FROM cards WHERE created_at >= NOW() - INTERVAL '30 days'
           GROUP BY day ORDER BY day"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // 补全 30 天空白天
    let fill_days = |rows: Vec<(chrono::NaiveDate, i64)>| -> Vec<Value> {
        let mut map: std::collections::HashMap<chrono::NaiveDate, i64> = rows.into_iter().collect();
        let mut filled = Vec::new();
        for i in (0..30).rev() {
            let d = (chrono::Utc::now() - chrono::Duration::days(i)).date_naive();
            let cnt = map.remove(&d).unwrap_or(0);
            filled.push(json!({"date": d.to_string(), "count": cnt}));
        }
        filled
    };

    Json(json!({
        "success": true,
        "data": {
            "merchants": fill_days(merchants),
            "apps": fill_days(apps),
            "cards": fill_days(cards),
        }
    }))
}

/// 获取当前管理员的 API Key
async fn get_admin_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let admin_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT api_key FROM admins WHERE id = $1",
    )
    .bind(admin_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let api_key = row.and_then(|(k,)| k).unwrap_or_default();
    Json(json!({"success": true, "data": { "api_key": api_key }}))
}

/// 重新生成管理员 API Key
async fn regenerate_admin_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let admin_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    let new_key = generate_api_key();
    let result = sqlx::query("UPDATE admins SET api_key = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_key)
        .bind(admin_id)
        .execute(&state.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({"success": true, "message": "API Key 已重新生成", "data": { "api_key": new_key }}))
        }
        Ok(_) => Json(json!({"success": false, "message": "管理员不存在"})),
        Err(e) => Json(json!({"success": false, "message": format!("生成失败: {}", e)})),
    }
}
