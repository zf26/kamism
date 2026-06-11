use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::oauth_config::{CreateOAuthProvider, OAuthConfig, OAuthConfigPublic, UpdateOAuthConfig},
};
use axum::{
    extract::{Path, State},
    middleware,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::{json, Value};

/// OAuth 配置默认值（各平台）
fn get_default_oauth_configs() -> Vec<(&'static str, &'static str, &'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "github",
            "GitHub",
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            "https://api.github.com/user",
            "user:email read:user",
        ),
        (
            "google",
            "Google",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://www.googleapis.com/oauth2/v2/userinfo",
            "email profile",
        ),
        (
            "microsoft",
            "Microsoft",
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            "https://graph.microsoft.com/oidc/userinfo",
            "openid email profile",
        ),
    ]
}

pub fn oauth_admin_router(state: AppState) -> Router<AppState> {
    Router::new()
        // OAuth 配置管理（管理员）
        .route("/admin/oauth/configs", get(list_oauth_configs))
        .route("/admin/oauth/configs", post(init_oauth_configs))
        .route("/admin/oauth/providers", post(create_oauth_provider))
        .route("/admin/oauth/configs/:provider", get(get_oauth_config))
        .route("/admin/oauth/configs/:provider", patch(update_oauth_config))
        .route("/admin/oauth/configs/:provider/toggle", post(toggle_oauth_config))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

/// 获取所有 OAuth 配置列表
async fn list_oauth_configs(State(state): State<AppState>) -> Json<Value> {
    let configs: Vec<OAuthConfigPublic> = sqlx::query_as(
        "SELECT id, provider, name, enabled, scopes FROM oauth_configs ORDER BY provider"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(json!({
        "success": true,
        "data": configs
    }))
}

/// 获取单个 OAuth 配置
async fn get_oauth_config(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Json<Value> {
    let config: Option<OAuthConfig> = sqlx::query_as(
        "SELECT * FROM oauth_configs WHERE provider = $1"
    )
    .bind(&provider)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    match config {
        Some(c) => {
            // 返回时隐藏 client_secret
            Json(json!({
                "success": true,
                "data": {
                    "id": c.id,
                    "provider": c.provider,
                    "name": c.name,
                    "client_id": c.client_id,
                    "client_secret_set": !c.client_secret.is_empty() && c.client_secret != "your_client_secret",
                    "redirect_uri": c.redirect_uri,
                    "auth_url": c.auth_url,
                    "token_url": c.token_url,
                    "userinfo_url": c.userinfo_url,
                    "scopes": c.scopes,
                    "enabled": c.enabled,
                    "extra_config": c.extra_config,
                }
            }))
        }
        None => Json(json!({
            "success": false,
            "message": "配置不存在"
        }))
    }
}

/// 初始化默认 OAuth 配置（如果不存在）
async fn init_oauth_configs(State(state): State<AppState>) -> Json<Value> {
    let defaults = get_default_oauth_configs();
    let mut created = 0;

    for (provider, name, auth_url, token_url, userinfo_url, scopes) in defaults {
        let exists: Option<(String,)> = sqlx::query_as(
            "SELECT id::text FROM oauth_configs WHERE provider = $1"
        )
        .bind(provider)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or(None);

        if exists.is_none() {
            let redirect_uri = format!("{}/oauth/{}/callback", state.app_url, provider);
            sqlx::query(
                "INSERT INTO oauth_configs (provider, name, client_id, client_secret, redirect_uri, auth_url, token_url, userinfo_url, scopes, enabled)
                 VALUES ($1, $2, 'your_client_id', 'your_client_secret', $3, $4, $5, $6, $7, FALSE)"
            )
            .bind(provider)
            .bind(name)
            .bind(&redirect_uri)
            .bind(auth_url)
            .bind(token_url)
            .bind(userinfo_url)
            .bind(scopes)
            .execute(&state.pool)
            .await
            .ok();
            created += 1;
        }
    }

    Json(json!({
        "success": true,
        "message": format!("已初始化 {} 个 OAuth 配置", created)
    }))
}

/// 创建自定义 OAuth 提供商
async fn create_oauth_provider(
    State(state): State<AppState>,
    Json(body): Json<CreateOAuthProvider>,
) -> Json<Value> {
    let provider_key = body.provider.trim().to_lowercase();

    // 校验：只允许字母、数字、连字符
    if !provider_key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Json(json!({
            "success": false,
            "message": "Provider ID 只能包含字母、数字、连字符和下划线"
        }));
    }

    // 校验：不能是空白或太长
    if provider_key.is_empty() || provider_key.len() > 32 {
        return Json(json!({
            "success": false,
            "message": "Provider ID 不能为空且最多 32 个字符"
        }));
    }

    // 校验 URL
    if !body.auth_url.starts_with("https://") && !body.auth_url.starts_with("http://") {
        return Json(json!({
            "success": false,
            "message": "授权地址必须是有效的 URL"
        }));
    }
    if !body.token_url.starts_with("https://") && !body.token_url.starts_with("http://") {
        return Json(json!({
            "success": false,
            "message": "Token 地址必须是有效的 URL"
        }));
    }
    if !body.userinfo_url.starts_with("https://")
        && !body.userinfo_url.starts_with("http://")
    {
        return Json(json!({
            "success": false,
            "message": "用户信息接口必须是有效的 URL"
        }));
    }

    // 检查是否已存在
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id::text FROM oauth_configs WHERE provider = $1")
            .bind(&provider_key)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    if exists.is_some() {
        return Json(json!({
            "success": false,
            "message": "该 Provider ID 已存在"
        }));
    }

    let redirect_uri = format!("{}/oauth/{}/callback", state.app_url, provider_key);
    let scopes = body.scopes.unwrap_or_default();

    let result = sqlx::query(
        "INSERT INTO oauth_configs (provider, name, client_id, client_secret, redirect_uri, auth_url, token_url, userinfo_url, scopes, enabled)
         VALUES ($1, $2, 'your_client_id', 'your_client_secret', $3, $4, $5, $6, $7, FALSE)",
    )
    .bind(&provider_key)
    .bind(&body.name)
    .bind(&redirect_uri)
    .bind(&body.auth_url)
    .bind(&body.token_url)
    .bind(&body.userinfo_url)
    .bind(&scopes)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "message": "创建成功"
            }))
        }
        Err(e) => Json(json!({
            "success": false,
            "message": format!("创建失败: {}", e)
        })),
        _ => Json(json!({
            "success": false,
            "message": "创建失败"
        })),
    }
}

/// 更新 OAuth 配置
async fn update_oauth_config(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Json(body): Json<UpdateOAuthConfig>,
) -> Json<Value> {
    let mut updates = Vec::new();
    let mut param_idx = 1;

    macro_rules! add_update {
        ($field:literal, $value:expr) => {
            if let Some(ref _v) = $value {
                updates.push(format!("{} = ${}", $field, param_idx));
                param_idx += 1;
            }
        };
    }

    add_update!("name", body.name);
    add_update!("client_id", body.client_id);
    add_update!("client_secret", body.client_secret);
    add_update!("redirect_uri", body.redirect_uri);
    add_update!("auth_url", body.auth_url);
    add_update!("token_url", body.token_url);
    add_update!("userinfo_url", body.userinfo_url);
    add_update!("scopes", body.scopes);
    let extra_config_exists = body.extra_config.is_some();
    if extra_config_exists {
        updates.push(format!("extra_config = ${}", param_idx));
        param_idx += 1;
    }
    add_update!("enabled", body.enabled);

    if updates.is_empty() {
        return Json(json!({
            "success": false,
            "message": "没有需要更新的字段"
        }));
    }

    updates.push("updated_at = NOW()".to_string());

    let query = format!(
        "UPDATE oauth_configs SET {} WHERE provider = ${}",
        updates.join(", "),
        param_idx
    );

    // 按 add_update! 声明顺序依次 .bind()，Rust 编译器会消除死代码
    let mut q = sqlx::query(&query);
    if body.name.is_some()             { q = q.bind(body.name.as_ref().unwrap()); }
    if body.client_id.is_some()       { q = q.bind(body.client_id.as_ref().unwrap()); }
    if body.client_secret.is_some()   { q = q.bind(body.client_secret.as_ref().unwrap()); }
    if body.redirect_uri.is_some()    { q = q.bind(body.redirect_uri.as_ref().unwrap()); }
    if body.auth_url.is_some()         { q = q.bind(body.auth_url.as_ref().unwrap()); }
    if body.token_url.is_some()        { q = q.bind(body.token_url.as_ref().unwrap()); }
    if body.userinfo_url.is_some()     { q = q.bind(body.userinfo_url.as_ref().unwrap()); }
    if body.scopes.is_some()           { q = q.bind(body.scopes.as_ref().unwrap()); }
    if let Some(ref v) = body.extra_config { q = q.bind(v.to_string()); }
    if body.enabled.is_some()          { q = q.bind(body.enabled.unwrap()); }
    let result = q.bind(&provider).execute(&state.pool).await;

    // 清除 OAuth 配置缓存
    state.invalidate_oauth_cache(Some(&provider)).await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "message": "配置已更新"
            }))
        }
        Ok(_) => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
        Err(e) => Json(json!({
            "success": false,
            "message": format!("更新失败: {}", e)
        }))
    }
}

/// 快速启用/禁用 OAuth 配置
async fn toggle_oauth_config(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let enabled = match body.get("enabled").and_then(|v| v.as_bool()) {
        Some(v) => v,
        None => return Json(json!({
            "success": false,
            "message": "缺少 enabled 参数"
        })),
    };

    let result = sqlx::query(
        "UPDATE oauth_configs SET enabled = $1, updated_at = NOW() WHERE provider = $2"
    )
    .bind(enabled)
    .bind(&provider)
    .execute(&state.pool)
    .await;

    // 清除缓存
    state.invalidate_oauth_cache(Some(&provider)).await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "message": if enabled { "已启用" } else { "已禁用" }
            }))
        }
        Ok(_) => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
        Err(e) => Json(json!({
            "success": false,
            "message": format!("操作失败: {}", e)
        }))
    }
}

