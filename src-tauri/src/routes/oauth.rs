use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::AppState,
    models::{merchant::Merchant, oauth_config::OAuthConfigPublic},
    utils::jwt::{generate_token, generate_refresh_token},
};
use axum::{
    extract::{Json, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
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
    let callback_base = if let Some(proto) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        // 优先用 x-forwarded-host，否则用 host
        let host = headers
            .get("x-forwarded-host")
            .and_then(|v| v.to_str().ok())
            .or_else(|| headers.get("host").and_then(|v| v.to_str().ok()));
        match host {
            Some(h) => format!("{}://{}", proto, h),
            None => state.app_url.clone(),
        }
    } else {
        // 非反向代理场景：直接用 host 或 app_url兜底
        headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(|h| format!("http://{}", h))
            .unwrap_or_else(|| state.app_url.clone())
    };

    let redirect_uri = format!("{}/api/oauth/{}/callback", callback_base, provider);

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

/// 构造 302 跳转。**永不 panic** —— 头值非法时记 error 并回 500。
///
/// ── 为什么需要这个函数 ──────────────────────────────────────────────────
///
/// 原先 6 处都是 `headers.insert(header::LOCATION, redirect_url.parse().unwrap())`。
/// `HeaderValue` 拒绝控制字符（CR/LF 等），而 `/oauth/:provider/callback` 的
/// `error` 参数是**直接从 query string 取出来、没有转义**就拼进 location 的：
///
/// ```text
/// GET /oauth/github/callback?error=%0d%0aX-Injected:%20yes
/// → [PANIC] called `Result::unwrap()` on an `Err` value: InvalidHeaderValue
///          at src-tauri\src\routes\oauth.rs:122:63
/// ```
///
/// 而且 `error` 分支在 `state` 校验**之前**就 return 了 —— 也就是**未登录可达、
/// 不受 CSRF 保护**，任何人都能用一个 GET 让服务端 panic 打一行日志，
/// 而客户端拿到的是**被直接掐断的连接**（连 500 都没有）。
///
/// 两层修法：
/// 1. 上面各调用点把插入 URL 的值做 `urlencoding::encode` —— 既堵住控制字符，
///    也修掉「`?error=中文` 未编码就进 Location」这个 URL 语义错误；
/// 2. 这里不再 `unwrap`：万一还失败（例如 `FRONTEND_URL` 本身配错），
///    记 error 回 500，绝不 panic。
fn redirect_to(location: String) -> Result<(StatusCode, HeaderMap, String), &'static str> {
    match HeaderValue::from_str(&location) {
        Ok(v) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::LOCATION, v);
            Ok((StatusCode::FOUND, headers, String::new()))
        }
        Err(e) => {
            // ⚠️ 只记长度，**不记 location 本身**：最后那个成功跳转的 location 里
            // 带着 token / refresh_token，写进日志等于把它们泄漏到日志文件里。
            tracing::error!(
                "OAuth 重定向地址不能作为 Location 头（已拒绝跳转并回 500）: err={} location_len={}",
                e,
                location.len()
            );
            Err("服务器繁忙，请稍后重试")
        }
    }
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
        // `error` 是 query string 里来的，必须转义 —— 它既可能含 CR/LF
        // （曾让这里的 `parse().unwrap()` panic），也可能含 `&` / `#`
        // （会破坏 Location 的 URL 语义，导致前端解析出错）。
        return redirect_to(format!(
            "{}/oauth/callback?error={}",
            frontend_url,
            urlencoding::encode(error)
        ));
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
        return redirect_to(format!("{}/oauth/callback?error=csrf", frontend_url));
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
            return redirect_to(format!("{}/oauth/callback?error=token", frontend_url));
        }
    };

    // 获取用户信息
    let (user_id, username, email, avatar) = match fetch_user_info(&config, &access_token).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Fetch user info failed: {}", e);
            return redirect_to(format!("{}/oauth/callback?error=userinfo", frontend_url));
        }
    };

    // 处理用户登录/注册
    let user_info = match handle_oauth_user(&state, &provider, &user_id, &username, &email, &avatar).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Handle OAuth user failed: {}", e);
            return redirect_to(format!("{}/oauth/callback?error=user", frontend_url));
        }
    };

    // 重定向到前端
    // ⚠️ 四个插值全部要 encode。原先 `role` 是裸拼的 —— 它是从库里读出来的
    // 枚举值（admin/merchant），眼下不含特殊字符，但「靠取值域干净」是运气而非保证；
    // 同一个 URL 里其它三个都 encode 了，只有它不 encode 是明显的疏漏。
    let redirect_url = format!(
        "{}/oauth/callback?token={}&refresh={}&role={}&user={}",
        frontend_url,
        urlencoding::encode(&user_info.token),
        urlencoding::encode(&user_info.refresh_token),
        urlencoding::encode(&user_info.role),
        urlencoding::encode(&serde_json::to_string(&user_info.user_info).unwrap_or_default()),
    );

    redirect_to(redirect_url)
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
        return generate_oauth_response(app_state, &merchant, "merchant").await;
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

        return generate_oauth_response(app_state, &merchant, "merchant").await;
    }

    // 新用户：自动注册
    let merchant_id = Uuid::new_v4();
    let unique_username = generate_unique_username(username);
    let api_key = crate::utils::card_gen::generate_api_key();
    // 占位密码：自动注册的 OAuth 用户没有可用密码，用随机 UUID 前 8 位当输入。
    // 这里 `[..8]` 是**按字节**切，但 `Uuid::to_string()` 的输出恒为 36 个 ASCII
    // 字符（8-4-4-4-12 的十六进制），字节数 == 字符数，所以不会踩 char boundary。
    // 第十二批统一审过全仓库的字节切片，这一处是「输入由构造保证 ASCII」的例外，
    // 不是漏网 —— 但**改这行前要先确认输入仍是 ASCII**。
    let password_hash = bcrypt::hash(&uuid::Uuid::new_v4().to_string()[..8], 10)
        .map_err(|_| "密码加密错误")?;

    let api_key_hash = EncryptedFieldsOps::generate_hash(&api_key);

    // 根据不同的 provider 设置对应的字段
    let (github_id, google_id, microsoft_id) = match provider {
        "github" => (Some(EncryptedFieldsOps::generate_hash(provider_user_id)), None, None),
        "google" => (None, Some(EncryptedFieldsOps::generate_hash(provider_user_id)), None),
        "microsoft" => (None, None, Some(EncryptedFieldsOps::generate_hash(provider_user_id))),
        _ => (None, None, None),
    };

    // ── 加密 + 插商户 + 记加密日志：同一个事务 ─────────────────────────────
    //
    // 与 `routes/auth.rs` 的注册流程同一个问题、同一个解法（那边有更长的说明）：
    // 旧写法用 `encrypt_merchant_api_key(&pool, ..)` / `encrypt_merchant_email(&pool, ..)`
    // **先把两条加密日志提交掉**，然后才 INSERT 商户 —— INSERT 一失败
    // （用户名冲突、provider 绑定列冲突……）就留下两条孤儿日志。
    // 现在日志跟着同一个事务走：失败一起消失，成功一起生效。
    let mut tx = app_state.pool.begin().await.map_err(|_| "数据库错误")?;

    let encrypted_api_key = EncryptedFieldsOps::encrypt_merchant_api_key_tx(
        &mut *tx,
        &app_state.encryptor,
        merchant_id,
        &api_key,
    )
    .await
    .map_err(|_| "加密错误")?;

    let encrypted_email = EncryptedFieldsOps::encrypt_merchant_email_tx(
        &mut *tx,
        &app_state.encryptor,
        merchant_id,
        email,
    )
    .await
    .map_err(|_| "加密错误")?;

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
    .execute(&mut *tx)
    .await
    .map_err(|_| "创建用户失败")?;

    // 提交失败也必须报错：否则会返回一个「登录成功、但账号其实没建成」的令牌，
    // 用户下一次刷新页面就变成一个查不到自己的账号（静默失败）。
    tx.commit().await.map_err(|_| "创建用户失败")?;

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
        // 全新用户，从未吊销过 —— 与 `009_token_version.sql` 的 DEFAULT 0 一致
        token_version: 0,
    };

    generate_oauth_response(app_state, &merchant, "merchant").await
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

async fn generate_oauth_response(
    app_state: &AppState,
    merchant: &Merchant,
    role: &str,
) -> Result<OAuthLoginResult, &'static str> {
    let email = EncryptedFieldsOps::decrypt_merchant_email(&app_state.encryptor, &merchant.email)
        .map_err(|_| "邮箱解密错误")?;

    let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(&app_state.encryptor, &merchant.api_key)
        .map_err(|_| "API Key 解密错误")?;

    // ── 吊销校验：OAuth 登录也必须走这一关 ──────────────────────────────────
    //
    // 这是一个容易被遗漏的点：OAuth 登录和密码登录走不同代码路径，
    // 只在密码登录里加吊销校验的话，「被禁用/改过密码的商户用 GitHub 登录」
    // 就会绕过去，拿到一张 ver 是最新值的全新令牌 —— 而那张令牌在
    // auth_middleware 里是**合法的**（因为 ver 与库一致）。
    //
    // 换句话说：**漏掉这里，等于给吊销开了个后门。**
    match crate::utils::jwt::check_token_version(
        &app_state.pool,
        &mut app_state.redis.clone(),
        role,
        &merchant.id,
        merchant.token_version as i64,
    )
    .await
    {
        crate::utils::jwt::VersionCheck::Valid => {}
        crate::utils::jwt::VersionCheck::Revoked { .. } => {
            return Err("登录状态已失效，请重新登录");
        }
        crate::utils::jwt::VersionCheck::Unavailable(e) => {
            tracing::error!("OAuth 登录时无法校验令牌版本，拒绝（fail-closed）: {}", e);
            return Err("服务暂时不可用，请稍后重试");
        }
    }

    let token = generate_token(&merchant.id, role, &email, merchant.token_version as i64, &app_state.jwt_secret)
        .map_err(|_| "Token 生成错误")?;

    let refresh_token = generate_refresh_token(&merchant.id, role, &email, merchant.token_version as i64, &app_state.jwt_secret)
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
            "plan": merchant.plan,
            "plan_expires_at": merchant.plan_expires_at,
            "email_verified": merchant.email_verified,
            "created_at": merchant.created_at
        }),
    })
}
