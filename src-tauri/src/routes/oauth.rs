use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::AppState,
    models::{merchant::Merchant, oauth_config::OAuthConfigPublic},
    utils::jwt::{generate_token, generate_refresh_token},
};
use axum::{
    extract::{Json, Query, State},
    http::{header, HeaderMap, StatusCode},
    routing::get,
    Router,
};
use rand::Rng;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

async fn list_enabled_providers(State(state): State<AppState>) -> Json<Value> {
    let configs: Vec<OAuthConfigPublic> = sqlx::query_as(
        "SELECT id, provider, name, enabled, scopes FROM oauth_configs WHERE enabled = TRUE"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(json!({
        "success": true,
        "data": configs
    }))
}

pub fn oauth_router(state: AppState) -> Router<AppState> {
    Router::new()
        // 公开路由：登录页面显示 OAuth 登录选项
        .route("/oauth/providers", get(list_enabled_providers))
        // OAuth 授权流程
        .route("/oauth/:provider/authorize", get(oauth_authorize))
        .route("/oauth/:provider/callback", get(oauth_callback))
        .with_state(state)
}

/// 获取 OAuth 授权 URL
async fn oauth_authorize(
    State(state): State<AppState>,
    axum::extract::Path(provider): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, &'static str> {
    let config = state.get_oauth_config(&provider).await
        .ok_or("该 OAuth 提供商未配置或未启用")?;

    let state_value = generate_random_state();

    // 优先用请求头中的 Host 动态构建回调地址（支持反向代理）
    let callback_base = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            headers
                .get("host")
                .and_then(|v| v.to_str().ok())
                .map(|h| format!("http://{}", h))
                .unwrap_or_else(|| state.app_url.clone())
        });

    let redirect_uri = format!("{}/oauth/{}/callback", callback_base, provider);

    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        config.auth_url,
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&config.scopes),
        urlencoding::encode(&state_value),
    );

    // 将 state 存入 Redis，5 分钟过期
    let mut redis = state.redis.clone();
    let state_key = format!("oauth:{}:state:{}", provider, state_value);
    let _: () = redis
        .set_ex(&state_key, &provider, 300)
        .await
        .map_err(|e| tracing::error!("Redis SET_EX error for key {}: {}", state_key, e))
        .map_err(|_| "Redis 错误")?;

    Ok(Json(serde_json::json!({ "url": auth_url })))
}

/// OAuth 回调处理
async fn oauth_callback(
    State(state): State<AppState>,
    axum::extract::Path(provider): axum::extract::Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
) -> Result<(StatusCode, HeaderMap, String), &'static str> {
    let frontend_url = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "http://localhost:1420".to_string());

    // 检查错误
    if let Some(error) = &params.error {
        tracing::warn!("OAuth error: {} - {:?}", error, params.error_description);
        let redirect_url = format!("{}/oauth/callback?error={}", frontend_url, error);
        let mut headers = HeaderMap::new();
        headers.insert(header::LOCATION, redirect_url.parse().unwrap());
        return Ok((StatusCode::FOUND, headers, String::new()));
    }

    let code = params.code.as_ref().ok_or("缺少授权码")?;
    let state_value = params.state.as_ref().ok_or("缺少 state 参数")?;

    // 验证 state
    let mut redis = state.redis.clone();
    let state_key = format!("oauth:{}:state:{}", provider, state_value);
    let stored: Option<String> = redis.get(&state_key).await
        .map_err(|e| tracing::error!("Redis GET error for key {}: {}", state_key, e))
        .map_err(|_| "Redis 错误")?;
    if stored.is_none() {
        let redirect_url = format!("{}/oauth/callback?error=csrf", frontend_url);
        let mut headers = HeaderMap::new();
        headers.insert(header::LOCATION, redirect_url.parse().unwrap());
        return Ok((StatusCode::FOUND, headers, String::new()));
    }
    let _: () = redis.del(&state_key).await
        .map_err(|e| tracing::error!("Redis DEL error for key {}: {}", state_key, e))
        .map_err(|_| "Redis 错误")?;

    // 获取配置
    let config = state.get_oauth_config(&provider).await
        .ok_or("OAuth 配置不存在")?;

    // 交换 token
    let access_token = match exchange_token(&config, code).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Token exchange failed: {}", e);
            let redirect_url = format!("{}/oauth/callback?error=token", frontend_url);
            let mut headers = HeaderMap::new();
            headers.insert(header::LOCATION, redirect_url.parse().unwrap());
            return Ok((StatusCode::FOUND, headers, String::new()));
        }
    };

    // 获取用户信息
    let (user_id, username, email, avatar) = match fetch_user_info(&config, &access_token).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Fetch user info failed: {}", e);
            let redirect_url = format!("{}/oauth/callback?error=userinfo", frontend_url);
            let mut headers = HeaderMap::new();
            headers.insert(header::LOCATION, redirect_url.parse().unwrap());
            return Ok((StatusCode::FOUND, headers, String::new()));
        }
    };

    // 处理用户登录/注册
    let user_info = match handle_oauth_user(&state, &provider, &user_id, &username, &email, &avatar).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Handle OAuth user failed: {}", e);
            let redirect_url = format!("{}/oauth/callback?error=user", frontend_url);
            let mut headers = HeaderMap::new();
            headers.insert(header::LOCATION, redirect_url.parse().unwrap());
            return Ok((StatusCode::FOUND, headers, String::new()));
        }
    };

    // 重定向到前端
    let redirect_url = format!(
        "{}/oauth/callback?token={}&refresh={}&role={}&user={}",
        frontend_url,
        urlencoding::encode(&user_info.token),
        urlencoding::encode(&user_info.refresh_token),
        user_info.role,
        urlencoding::encode(&serde_json::to_string(&user_info.user_info).unwrap_or_default()),
    );

    let mut headers = HeaderMap::new();
    headers.insert(header::LOCATION, redirect_url.parse().unwrap());
    Ok((StatusCode::FOUND, headers, String::new()))
}

/// 交换授权码为 access token
async fn exchange_token(
    config: &crate::models::oauth_config::OAuthConfig,
    code: &str,
) -> anyhow::Result<String> {
    let client = reqwest::Client::new();

    let params = [
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
        ("code", code),
        ("redirect_uri", config.redirect_uri.as_str()),
    ];

    let resp = client
        .post(&config.token_url)
        .header("Accept", "application/json")
        .header("User-Agent", "kamism-server")
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Token exchange failed: {}", resp.status());
    }

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
    }

    let token_resp: TokenResponse = resp.json().await?;
    Ok(token_resp.access_token)
}

/// 获取用户信息（通用实现，各平台字段可能不同）
async fn fetch_user_info(
    config: &crate::models::oauth_config::OAuthConfig,
    access_token: &str,
) -> anyhow::Result<(String, String, String, Option<String>)> {
    let client = reqwest::Client::new();
    let resp = client
        .get(&config.userinfo_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .header("User-Agent", "kamism-server")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("User info request failed: {}", resp.status());
    }

    let body: serde_json::Value = resp.json().await?;

    // 尝试解析不同的字段格式
    let user_id = body.get("id")
        .and_then(|v| v.as_i64())
        .map(|id| id.to_string())
        .or_else(|| body.get("sub").and_then(|v| v.as_str().map(String::from)))
        .ok_or_else(|| anyhow::anyhow!("Cannot find user id"))?;

    let username = body.get("login")
        .or_else(|| body.get("username"))
        .or_else(|| body.get("name"))
        .or_else(|| body.get("preferred_username"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("user_{}", &user_id[..8.min(user_id.len())]));

    let mut email = body.get("email")
        .and_then(|v| v.as_str())
        .map(String::from);

    // 如果 userinfo 没有返回 email（常见于未公开/未验证邮箱），则请求 emails 列表
    if email.is_none() {
        let client = reqwest::Client::new();
        let emails_resp = client
            .get(&config.userinfo_url.replace("/user", "/user/emails"))
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Accept", "application/json")
            .header("User-Agent", "kamism-server")
            .send()
            .await;

        if let Ok(emails_resp) = emails_resp {
            if emails_resp.status().is_success() {
                if let Ok(emails) = emails_resp.json::<Vec<serde_json::Value>>().await {
                    email = emails
                        .iter()
                        .find(|e| e.get("primary").and_then(|v| v.as_bool()).unwrap_or(false)
                              && e.get("verified").and_then(|v| v.as_bool()).unwrap_or(false))
                        .and_then(|e| e.get("email").and_then(|v| v.as_str()))
                        .map(String::from)
                        .or_else(|| {
                            emails.iter()
                                .find(|e| e.get("verified").and_then(|v| v.as_bool()).unwrap_or(false))
                                .and_then(|e| e.get("email").and_then(|v| v.as_str()))
                                .map(String::from)
                        });
                }
            }
        }
    }

    let email = email.ok_or_else(|| anyhow::anyhow!("Cannot find email"))?;

    let avatar = body.get("avatar_url")
        .or_else(|| body.get("picture"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok((user_id, username, email, avatar))
}

fn generate_random_state() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

struct OAuthLoginResult {
    token: String,
    refresh_token: String,
    role: String,
    user_info: Value,
}

async fn handle_oauth_user(
    app_state: &AppState,
    provider: &str,
    provider_user_id: &str,
    username: &str,
    email: &str,
    _avatar: &Option<String>,
) -> Result<OAuthLoginResult, &'static str> {
    let binding_field = format!("{}_id", provider);
    let binding_column = binding_field.as_str();

    // 检查是否已绑定过
    let existing: Option<Merchant> = sqlx::query_as(&format!(
        "SELECT * FROM merchants WHERE {} = $1 LIMIT 1",
        binding_column
    ))
    .bind(provider_user_id)
    .fetch_optional(&app_state.pool)
    .await
    .map_err(|_| "数据库错误")?;

    if let Some(merchant) = existing {
        return generate_oauth_response(app_state, &merchant, "merchant");
    }

    // 通过邮箱查找已有账号
    let email_hash = EncryptedFieldsOps::generate_hash(email);
    let by_email: Option<Merchant> = sqlx::query_as(
        "SELECT * FROM merchants WHERE email_hash = $1 LIMIT 1",
    )
    .bind(&email_hash)
    .fetch_optional(&app_state.pool)
    .await
    .map_err(|_| "数据库错误")?;

    if let Some(mut merchant) = by_email {
        // 已有账号：绑定 OAuth
        sqlx::query(&format!(
            "UPDATE merchants SET {} = $1, updated_at = NOW() WHERE id = $2",
            binding_column
        ))
        .bind(provider_user_id)
        .bind(merchant.id)
        .execute(&app_state.pool)
        .await
        .map_err(|_| "数据库错误")?;

        let binding = EncryptedFieldsOps::generate_hash(provider_user_id);
        merchant.github_id = if binding_column == "github_id" { Some(binding) } else { merchant.github_id };

        return generate_oauth_response(app_state, &merchant, "merchant");
    }

    // 新用户：自动注册
    let merchant_id = Uuid::new_v4();
    let unique_username = generate_unique_username(username);
    let api_key = crate::utils::card_gen::generate_api_key();
    let password_hash = bcrypt::hash(&uuid::Uuid::new_v4().to_string()[..8], 10)
        .map_err(|_| "密码加密错误")?;

    let api_key_hash = EncryptedFieldsOps::generate_hash(&api_key);
    let encrypted_api_key = EncryptedFieldsOps::encrypt_merchant_api_key(
        &app_state.pool,
        &app_state.encryptor,
        merchant_id,
        &api_key,
    )
    .await
    .map_err(|_| "加密错误")?;

    let encrypted_email = EncryptedFieldsOps::encrypt_merchant_email(
        &app_state.pool,
        &app_state.encryptor,
        merchant_id,
        email,
    )
    .await
    .map_err(|_| "加密错误")?;

    // 根据不同的 provider 设置对应的字段
    let (github_id, google_id, microsoft_id) = match provider {
        "github" => (Some(EncryptedFieldsOps::generate_hash(provider_user_id)), None, None),
        "google" => (None, Some(EncryptedFieldsOps::generate_hash(provider_user_id)), None),
        "microsoft" => (None, None, Some(EncryptedFieldsOps::generate_hash(provider_user_id))),
        _ => (None, None, None),
    };

    sqlx::query(
        "INSERT INTO merchants (id, username, email_encrypted, email_hash, github_id, google_id, microsoft_id, password_hash, api_key_encrypted, api_key_hash, email_verified, status, plan)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, TRUE, 'active', 'free')",
    )
    .bind(merchant_id)
    .bind(&unique_username)
    .bind(&encrypted_email)
    .bind(&email_hash)
    .bind(&github_id)
    .bind(&google_id)
    .bind(&microsoft_id)
    .bind(&password_hash)
    .bind(&encrypted_api_key)
    .bind(&api_key_hash)
    .execute(&app_state.pool)
    .await
    .map_err(|_| "创建用户失败")?;

    let merchant = Merchant {
        id: merchant_id,
        username: unique_username,
        email: encrypted_email,
        email_hash,
        github_id,
        google_id,
        microsoft_id,
        password_hash,
        api_key: encrypted_api_key,
        api_key_hash,
        status: "active".to_string(),
        plan: "free".to_string(),
        plan_expires_at: None,
        email_verified: true,
        verify_token: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        created_by_admin: false,
    };

    generate_oauth_response(app_state, &merchant, "merchant")
}

fn generate_unique_username(base: &str) -> String {
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .take(20)
        .collect();

    let cleaned = if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    };

    let suffix: u16 = rand::thread_rng().gen();
    format!("{}_{}", cleaned, suffix)
}

fn generate_oauth_response(
    app_state: &AppState,
    merchant: &Merchant,
    role: &str,
) -> Result<OAuthLoginResult, &'static str> {
    let email = EncryptedFieldsOps::decrypt_merchant_email(&app_state.encryptor, &merchant.email)
        .map_err(|_| "邮箱解密错误")?;

    let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(&app_state.encryptor, &merchant.api_key)
        .map_err(|_| "API Key 解密错误")?;

    let token = generate_token(&merchant.id, role, &email, &app_state.jwt_secret)
        .map_err(|_| "Token 生成错误")?;

    let refresh_token = generate_refresh_token(&merchant.id, role, &email, &app_state.jwt_secret)
        .map_err(|_| "Refresh Token 生成错误")?;

    Ok(OAuthLoginResult {
        token,
        refresh_token,
        role: role.to_string(),
        user_info: json!({
            "id": merchant.id,
            "username": merchant.username,
            "email": email,
            "api_key": api_key,
            "status": merchant.status,
            "email_verified": merchant.email_verified,
            "created_at": merchant.created_at
        }),
    })
}
