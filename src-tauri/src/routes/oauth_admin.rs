use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::oauth_config::{CreateOAuthProvider, OAuthConfig, OAuthConfigPublic, UpdateOAuthConfig},
    utils::db_guard,
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
    // ⚠️ 曾用 .unwrap_or_default()：查询失败时 OAuth 配置列表变空，
    // 管理员看到「暂无 OAuth 配置」，会以为登录渠道全被删了并去重新初始化，
    // 实际是数据库没查出来 —— 初始化还会把真实配置覆盖掉。
    let configs: Vec<OAuthConfigPublic> = match sqlx::query_as(
        "SELECT id, provider, name, enabled, scopes FROM oauth_configs ORDER BY provider"
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询 OAuth 配置列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

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
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 前端显示「该 Provider 未配置」。
    // 管理员看到空的表单，会以为配置丢了并重新填一遍 —— 真正的问题（数据库）
    // 从来没被暴露。
    let config = match db_guard::optional(
        sqlx::query_as::<_, OAuthConfig>("SELECT * FROM oauth_configs WHERE provider = $1")
            .bind(&provider)
            .fetch_optional(&state.pool),
        "查询 OAuth Provider 配置",
    )
    .await
    {
        db_guard::QueryOutcome::Found(c) => Some(c),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

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
    let mut failed = 0;

    for (provider, name, auth_url, token_url, userinfo_url, scopes) in defaults {
        // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 判定「不存在」→ **执行 INSERT**。
        // 这条路是初始化默认配置用的，重复插入会撞 UNIQUE 约束；
        // 但更糟的是「因为查不到就去创建」这个逻辑本身 —— 把「查不了」
        // 当成了「没有」，然后**写库**。
        let exists = match db_guard::optional(
            sqlx::query_as::<_, (String,)>("SELECT id::text FROM oauth_configs WHERE provider = $1")
                .bind(provider)
                .fetch_optional(&state.pool),
            "检查 OAuth Provider 是否已配置（初始化默认值）",
        )
        .await
        {
            db_guard::QueryOutcome::Found(v) => Some(v),
            db_guard::QueryOutcome::NotFound => None,
            db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
        };

        if exists.is_none() {
            let redirect_uri = format!("{}/oauth/{}/callback", state.app_url, provider);
            // ⚠️ 曾经这里是 `.execute(...).await.ok(); created += 1;` —— 两处都错：
            //
            // 1. 失败被 `.ok()` 吞掉（INSERT 撞 UNIQUE、连接断开、字段超长…都算）；
            // 2. 计数写在 match 外面 —— **失败也照样 `created += 1`**。
            //
            // 合起来的效果：管理员点「初始化」，界面显示「已初始化 4 个 OAuth 配置」，
            // 而后台可能一个都没建出来。这是最坏的一类报错 ——
            // 它不只是不说真话，而是**说了一句让人放心的话**，管理员看到之后
            // 就不会再去查了。
            //
            // 这里不做整体回滚（不开事务）：每个 provider 的插入互相独立，
            // 部分成功是有意义的状态 —— 更关键的是这个接口**天然幂等可重入**，
            // 已存在的会被上面的 `exists` 检查跳过，再点一次就能把漏掉的补上。
            // 前提是计数如实反映结果，否则管理员不知道自己该不该再点一次。
            let insert = sqlx::query(
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
            .await;
            match insert {
                Ok(_) => created += 1,
                Err(e) => {
                    tracing::error!(
                        "初始化 OAuth 配置失败（该 provider 未创建，可重试）: provider={} err={}",
                        provider,
                        e
                    );
                    failed += 1;
                }
            }
        }
    }

    // 有失败就不能报 success —— 上面的 message 已经把数量说清楚了，
    // 这里只是不让前端把「部分失败」渲染成一个绿色的成功提示。
    if failed > 0 {
        return Json(json!({
            "success": false,
            "message": format!("已初始化 {} 个 OAuth 配置，{} 个失败（详见服务端日志，可重试）", created, failed)
        }));
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
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 判定「可以创建」→ 继续往下 INSERT。
    // 与 auth.rs 的注册查重同一类问题：绕过了唯一性检查。
    let exists = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM oauth_configs WHERE provider = $1")
            .bind(&provider_key)
            .fetch_optional(&state.pool),
        "检查 OAuth Provider ID 是否已存在",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

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
        Err(e) => db_guard::internal_error("创建 OAuth 提供商", e),
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
        Err(e) => db_guard::internal_error("更新 OAuth 配置", e)
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
        Err(e) => db_guard::internal_error("OAuth 配置操作", e)
    }
}

