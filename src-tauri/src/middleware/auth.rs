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

#[derive(Clone)]
pub struct AppState {
    pub pool: crate::db::DbPool,
    pub jwt_secret: String,
    pub mailer: MailerConfig,
    pub redis: ConnectionManager,
    pub mq_channel: std::sync::Arc<Channel>,
    pub encryptor: std::sync::Arc<Encryptor>,
    pub ws_registry: WsRegistry,
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
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"success": false, "message": "令牌无效或已过期"})),
        )
            .into_response(),
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
