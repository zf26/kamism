use crate::models::oauth_config::OAuthConfig;
use crate::models::payment_config::PaymentConfig;
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
        let config: Option<OAuthConfig> = sqlx::query_as(
            "SELECT * FROM oauth_configs WHERE provider = $1 AND enabled = TRUE"
        )
        .bind(provider)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

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

        let config: Option<PaymentConfig> = sqlx::query_as(
            "SELECT * FROM payment_configs WHERE channel = $1 AND enabled = TRUE",
        )
        .bind(channel)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(ref c) = config {
            let mut cache = self.payment_config_cache.write().await;
            cache.insert(channel.to_string(), c.clone());
        }

        config
    }

    pub async fn get_payment_config_any(&self, channel: &str) -> Option<PaymentConfig> {
        {
            let cache = self.payment_config_cache.read().await;
            if let Some(config) = cache.get(channel) {
                return Some(config.clone());
            }
        }

        let config: Option<PaymentConfig> = sqlx::query_as(
            "SELECT * FROM payment_configs WHERE channel = $1",
        )
        .bind(channel)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(ref c) = config {
            let mut cache = self.payment_config_cache.write().await;
            cache.insert(channel.to_string(), c.clone());
        }

        config
    }

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
            req.extensions_mut().insert(claims.clone());
            let resp = next.run(req).await;
            resp
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
