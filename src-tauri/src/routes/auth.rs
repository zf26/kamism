use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::{
        auth::AppState,
        rate_limit::login_rate_limit,
    },
    models::merchant::Merchant,
    utils::{
        card_gen::generate_api_key,
        jwt::{generate_token, generate_refresh_token, verify_refresh_token},
        mailer::send_verify_code,
    },
};
use axum::{
    extract::State,
    middleware,
    routing::post,
    Json, Router,
};
use bcrypt::{hash, verify, DEFAULT_COST};
use rand::Rng;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub code: String,
}

#[derive(Deserialize)]
pub struct SendCodeRequest {
    pub email: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

pub fn auth_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/auth/send-code", post(send_code))
        .route("/auth/register", post(register))
        .route("/auth/refresh", post(refresh_token))
        .route(
            "/auth/login",
            post(login).route_layer(
                middleware::from_fn_with_state(state, login_rate_limit)
            ),
        )
}

/// 发送注册验证码
async fn send_code(
    State(state): State<AppState>,
    Json(body): Json<SendCodeRequest>,
) -> Json<Value> {
    if !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }

    // 检查邮箱是否已注册（使用哈希索引查询）
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id::text FROM merchants WHERE email_hash = $1 LIMIT 1")
            .bind(&email_hash)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);
    if exists.is_some() {
        return Json(json!({"success": false, "message": "该邮箱已注册"}));
    }

    let mut redis = state.redis.clone();
    let cooldown_key = format!("code:cooldown:{}", body.email);

    // 60秒冷却防刷
    let in_cooldown: bool = redis.exists(&cooldown_key).await.unwrap_or(false);
    if in_cooldown {
        return Json(json!({"success": false, "message": "请求过于频繁，请60秒后再试"}));
    }

    // 生成6位数字验证码
    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Uniform::new(0u32, 10))
        .take(6)
        .map(|d| char::from_digit(d, 10).unwrap())
        .collect();

    // 存入 Redis：验证码 10 分钟过期，冷却标记 60 秒过期
    let code_key = format!("code:verify:{}", body.email);
    let _: () = redis.set_ex(&code_key, &code, 600).await.unwrap_or(());
    let _: () = redis.set_ex(&cooldown_key, "1", 60).await.unwrap_or(());

    // 发送邮件
    match send_verify_code(&state.mailer, &body.email, &code).await {
        Ok(_) => Json(json!({"success": true, "message": "验证码已发送，请查收邮件"})),
        Err(e) => {
            tracing::error!("发送验证码邮件失败: {}", e);
            // 发送失败则清除 Redis 记录，允许重试
            let _: () = redis.del(&code_key).await.unwrap_or(());
            let _: () = redis.del(&cooldown_key).await.unwrap_or(());
            Json(json!({"success": false, "message": "邮件发送失败，请稍后重试"}))
        }
    }
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Json<Value> {
    if !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }
    if body.password.len() < 8 {
        return Json(json!({"success": false, "message": "密码至少8位"}));
    }
    if body.username.len() < 3 {
        return Json(json!({"success": false, "message": "用户名至少3位"}));
    }
    if body.code.len() != 6 || !body.code.chars().all(|c| c.is_ascii_digit()) {
        return Json(json!({"success": false, "message": "验证码格式错误"}));
    }

    let mut redis = state.redis.clone();
    let code_key = format!("code:verify:{}", body.email);

    // 从 Redis 取出验证码
    let stored_code: Option<String> = redis.get(&code_key).await.unwrap_or(None);
    match stored_code {
        None => return Json(json!({"success": false, "message": "验证码无效或已过期"})),
        Some(c) if c != body.code => return Json(json!({"success": false, "message": "验证码错误"})),
        Some(_) => {
            // 验证通过，立即删除（一次性）
            let _: () = redis.del(&code_key).await.unwrap_or(());
        }
    }

    // 检查用户名是否已存在
    let exists: Option<(String,)> = sqlx::query_as(
        "SELECT id::text FROM merchants WHERE username = $1 LIMIT 1",
    )
    .bind(&body.username)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if exists.is_some() {
        return Json(json!({"success": false, "message": "用户名已存在"}));
    }
    
    // 检查邮箱是否已注册（使用哈希索引查询）
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let email_exists: Option<(String,)> = sqlx::query_as(
        "SELECT id::text FROM merchants WHERE email_hash = $1 LIMIT 1",
    )
    .bind(&email_hash)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if email_exists.is_some() {
        return Json(json!({"success": false, "message": "邮箱已存在"}));
    }

    let password_hash = match hash(&body.password, DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    let api_key = generate_api_key();
    let merchant_id = Uuid::new_v4();

    // 生成哈希值
    let api_key_hash = EncryptedFieldsOps::generate_hash(&api_key);
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);

    // 加密 API Key 和邮箱
    let encrypted_api_key = match EncryptedFieldsOps::encrypt_merchant_api_key(
        &state.pool,
        &state.encryptor,
        merchant_id,
        &api_key,
    ).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("加密 API Key 失败: {}", e);
            return Json(json!({"success": false, "message": "注册失败"}));
        }
    };

    let encrypted_email = match EncryptedFieldsOps::encrypt_merchant_email(
        &state.pool,
        &state.encryptor,
        merchant_id,
        &body.email,
    ).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("加密邮箱失败: {}", e);
            return Json(json!({"success": false, "message": "注册失败"}));
        }
    };

    let result = sqlx::query(
        "INSERT INTO merchants (id, username, email_encrypted, email_hash, password_hash, api_key_encrypted, api_key_hash, email_verified) VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE)",
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
        Ok(_) => Json(json!({"success": true, "message": "注册成功，请登录"})),
        Err(e) => Json(json!({"success": false, "message": format!("注册失败: {}", e)})),
    }
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Json<Value> {
    // 先查管理员表
    let admin: Option<crate::models::admin::Admin> =
        sqlx::query_as("SELECT * FROM admins WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    if let Some(admin) = admin {
        let valid = verify(&body.password, &admin.password_hash).unwrap_or(false);
        if !valid {
            return Json(json!({"success": false, "message": "邮箱或密码错误"}));
        }
        let token = match generate_token(&admin.id, "admin", &admin.email, &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
        };
        let refresh_token = match generate_refresh_token(&admin.id, "admin", &admin.email, &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
        };
        return Json(json!({
            "success": true,
            "token": token,
            "refresh_token": refresh_token,
            "role": "admin",
            "user": {
                "id": admin.id,
                "username": admin.username,
                "email": admin.email
            }
        }));
    }

    // 再查商户表（使用哈希索引查询）
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let merchant: Option<Merchant> =
        sqlx::query_as("SELECT * FROM merchants WHERE email_hash = $1")
            .bind(&email_hash)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let merchant = match merchant {
        Some(m) => m,
        None => return Json(json!({"success": false, "message": "邮箱或密码错误"})),
    };

    if merchant.status == "disabled" {
        return Json(json!({"success": false, "message": "账号已被禁用"}));
    }

    let valid = verify(&body.password, &merchant.password_hash).unwrap_or(false);
    if !valid {
        return Json(json!({"success": false, "message": "邮箱或密码错误"}));
    }

    let token = match generate_token(&merchant.id, "merchant", &merchant.email, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };
    let refresh_token = match generate_refresh_token(&merchant.id, "merchant", &merchant.email, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };

    // 解密 API Key 和邮箱
    let api_key = match EncryptedFieldsOps::decrypt_merchant_api_key(&state.encryptor, &merchant.api_key) {
        Ok(key) => key,
        Err(e) => {
            tracing::error!("解密 API Key 失败: {}", e);
            return Json(json!({"success": false, "message": "解密失败"}));
        }
    };

    let email = match EncryptedFieldsOps::decrypt_merchant_email(&state.encryptor, &merchant.email) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("解密邮箱失败: {}", e);
            return Json(json!({"success": false, "message": "解密失败"}));
        }
    };

    Json(json!({
        "success": true,
        "token": token,
        "refresh_token": refresh_token,
        "role": "merchant",
        "user": {
            "id": merchant.id,
            "username": merchant.username,
            "email": email,
            "api_key": api_key,
            "status": merchant.status,
            "email_verified": merchant.email_verified,
            "created_at": merchant.created_at
        }
    }))
}

async fn refresh_token(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Json<Value> {
    let claims = match verify_refresh_token(&body.refresh_token, &state.jwt_secret) {
        Ok(c) => c,
        Err(_) => return Json(json!({"success": false, "message": "Refresh Token 无效或已过期，请重新登录"})),
    };

    let user_id = match uuid::Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    // 验证用户账号仍然有效
    let still_active = if claims.role == "admin" {
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM admins WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None)
            .is_some()
    } else {
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE id = $1 AND status = 'active'")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None)
            .is_some()
    };

    if !still_active {
        return Json(json!({"success": false, "message": "账号不存在或已被禁用"}));
    }

    // 签发新 Access Token
    let new_token = match generate_token(&user_id, &claims.role, &claims.email, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };

    // 同时滚动续期 Refresh Token
    let new_refresh = match generate_refresh_token(&user_id, &claims.role, &claims.email, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };

    Json(json!({
        "success": true,
        "token": new_token,
        "refresh_token": new_refresh
    }))
}
