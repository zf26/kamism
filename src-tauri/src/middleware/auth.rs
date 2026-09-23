use crate::models::oauth_config::OAuthConfig;
use crate::models::payment_config::PaymentConfig;
use crate::utils::db_guard;
use crate::utils::jwt::{verify_token, Claims};
use crate::utils::kms::Encryptor;
use crate::utils::mailer::MailerConfig;
use crate::utils::ws::WsRegistry;
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use lapin::Channel;
use redis::aio::ConnectionManager;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub type OAuthConfigCache = Arc<RwLock<HashMap<String, OAuthConfig>>>;
pub type PaymentConfigCache = Arc<RwLock<HashMap<String, PaymentConfig>>>;

#[derive(Clone)]
pub struct AppState {
    pub pool: crate::db::DbPool,
    pub jwt_secret: String,
    pub mailer: MailerConfig,
    pub redis: ConnectionManager,
    pub mq_channel: std::sync::Arc<Channel>,
    pub encryptor: std::sync::Arc<Encryptor>,
    pub ws_registry: WsRegistry,
    pub oauth_config_cache: OAuthConfigCache,
    pub payment_config_cache: PaymentConfigCache,
    pub app_url: String,
    /// 可信代理策略：决定「客户端 IP」到底取谁（见 utils::client_ip 的说明）
    pub trusted_proxies: crate::utils::client_ip::TrustedProxies,
}

impl AppState {
    pub async fn get_oauth_config(&self, provider: &str) -> Option<OAuthConfig> {
        // 先从缓存读取
        {
            let cache = self.oauth_config_cache.read().await;
            if let Some(config) = cache.get(provider) {
                return Some(config.clone());
            }
        }

        // 缓存未命中，从数据库加载
        //
        // ⚠️ 这里把「查不了」并入了「没配置」—— 对调用方（`oauth.rs`）二者确实是
        // 同一个降级结果，都会回「该 OAuth 提供商未配置或未启用」，所以合并**是有意的**。
        // 但原来的 `.ok().flatten()` 让这次降级彻底无声：数据库一抖，
        // 用户在登录页看到「未启用」，管理员去后台一看配置好好的 ——
        // 排查方向直接跑偏。`optional_lenient` 保留了合并，但留下 warn 日志。
        //
        // 另外：失败**不会**被下面写进缓存（只 cache `Some`），所以数据库恢复后自动转好，
        // 不会把这个「未配置」的假象固化下来。
        let config: Option<OAuthConfig> = db_guard::optional_lenient(
            sqlx::query_as(
                "SELECT * FROM oauth_configs WHERE provider = $1 AND enabled = TRUE"
            )
            .bind(provider)
            .fetch_optional(&self.pool),
            "加载 OAuth 配置（失败则本次请求里该 provider 表现为「未启用」）",
        )
        .await;

        if let Some(ref c) = config {
            let mut cache = self.oauth_config_cache.write().await;
            cache.insert(provider.to_string(), c.clone());
        }

        config
    }

    pub async fn invalidate_oauth_cache(&self, provider: Option<&str>) {
        let mut cache = self.oauth_config_cache.write().await;
        if let Some(p) = provider {
            cache.remove(p);
        } else {
            cache.clear();
        }
    }

    pub async fn get_payment_config(&self, channel: &str) -> Option<PaymentConfig> {
        {
            let cache = self.payment_config_cache.read().await;
            if let Some(config) = cache.get(channel) {
                return Some(config.clone());
            }
        }

        // 同 `get_oauth_config`：合并「查不了」与「没启用」是有意的降级
        // （`payments.rs` 会回「XX 未配置」），但必须留痕。
        // 尤其在这里 —— 支付页上「未配置」这三个字会让管理员去后台反复确认配置，
        // 而真相很可能是数据库连不上。
        let config: Option<PaymentConfig> = db_guard::optional_lenient(
            sqlx::query_as(
                "SELECT * FROM payment_configs WHERE channel = $1 AND enabled = TRUE",
            )
            .bind(channel)
            .fetch_optional(&self.pool),
            "加载支付渠道配置（失败则本次请求里该渠道表现为「未配置」）",
        )
        .await;

        if let Some(ref c) = config {
            let mut cache = self.payment_config_cache.write().await;
            cache.insert(channel.to_string(), c.clone());
        }

        config
    }

    // ── 这里曾有一个 `get_payment_config_any`（不带 `enabled = TRUE`），已删 ──
    //
    // 为什么删：它没有任何调用点（全仓库 grep 确认），却有一个**隐蔽的坑** ——
    // 它和上面的 `get_payment_config` 往**同一个 `payment_config_cache`、同一个
    // key（channel）** 写。两个函数的语义相反：上面只看「启用中」的配置，
    // 它看「不管启不启用」的配置。一旦有人接上它，就会出现这种串味：
    // 先调 `get_payment_config_any`（把一条 disabled 配置写进缓存），
    // 再调 `get_payment_config` 就会命中那条 disabled 配置 ——
    // 支付环节拿到一个本不该启用的通道，且极难排查（缓存里看起来一切正常）。
    //
    // 所以：**不要把这个函数加回来**。如果将来确实需要「查未启用的配置」，
    // 要么给它一个独立的缓存 key（如 `channel + ":any"`），要么单独查库不写缓存。
    pub async fn invalidate_payment_cache(&self, channel: Option<&str>) {
        let mut cache = self.payment_config_cache.write().await;
        if let Some(c) = channel {
            cache.remove(c);
        } else {
            cache.clear();
        }
    }
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"success": false, "message": "缺少认证令牌"})),
            )
                .into_response();
        }
    };

    match verify_token(token, &state.jwt_secret) {
        Ok(claims) => {
            // ── 令牌版本校验（吊销机制）──────────────────────────────────────
            //
            // 光验签是不够的：JWT 是无状态的，签名对就永远对。
            // 「改密码 / 重置 / 被禁用 / 重置 api_key」都必须能作废已签发的令牌，
            // 否则封号封不住、改密等于没改。
            //
            // 判定逻辑在 utils::jwt::check_token_version，它返回三态而不是 bool ——
            // **不要把它简化成 bool**，理由见那个枚举的注释：
            // 「已吊销」要拒绝，「基础设施故障」是另一回事。
            let user_id = match Uuid::parse_str(&claims.sub) {
                Ok(id) => id,
                Err(_) => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"success": false, "message": "令牌无效或已过期"})),
                    )
                        .into_response()
                }
            };

            match crate::utils::jwt::check_token_version(
                &state.pool,
                &mut state.redis.clone(),
                &claims.role,
                &user_id,
                claims.ver,
            )
            .await
            {
                crate::utils::jwt::VersionCheck::Valid => {
                    req.extensions_mut().insert(claims.clone());
                    next.run(req).await
                }
                crate::utils::jwt::VersionCheck::Revoked { .. } => {
                    // 明确拒绝：这个令牌已经被「改密/重置/禁用」作废了
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({
                            "success": false,
                            "message": "登录状态已失效，请重新登录"
                        })),
                    )
                        .into_response()
                }
                crate::utils::jwt::VersionCheck::Unavailable(e) => {
                    // ⚠️ 这里是本函数最需要想清楚的一个分支。
                    //
                    // 库和 Redis 同时不可用 = 我们**无法判定**这个令牌是否已被吊销。
                    // 两个方向的错误后果不对称：
                    //   - 放行（fail-open）→ 吊销失效。攻击者若配合「把库打挂」
                    //     就能绕过吊销；不过更现实的是：库挂了整个服务本来也做不了事。
                    //   - 拒绝（fail-closed）→ 库一抖，全站所有人被登出。
                    //
                    // 选 **fail-closed（拒绝）**：这是安全边界。
                    // 一个无法判定授权的请求不应该被当成「已授权」。
                    // 而且此时数据库不可用，绝大多数接口本来也返回不了正确结果 ——
                    // 拒绝只是让失败来得更早、更清楚，而不是更晚、更奇怪。
                    tracing::error!("令牌版本校验无法完成，拒绝请求（fail-closed）: {}", e);
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "success": false,
                            "message": "服务暂时不可用，请稍后重试"
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(_e) => {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"success": false, "message": "令牌无效或已过期"})),
            )
                .into_response()
        }
    }
}

pub async fn admin_only(req: Request, next: Next) -> Response {
    let claims = req.extensions().get::<Claims>().cloned();
    match claims {
        Some(c) if c.role == "admin" => next.run(req).await,
        _ => (
            StatusCode::FORBIDDEN,
            Json(json!({"success": false, "message": "需要管理员权限"})),
        )
            .into_response(),
    }
}
