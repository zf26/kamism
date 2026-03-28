//! 基于 Redis 的速率限制
//! 使用滑动窗口计数器：key 在窗口期内计数，超出阈值返回 429

use crate::middleware::auth::AppState;
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use redis::AsyncCommands;
use serde_json::json;
use std::net::SocketAddr;

/// 登录接口限流：同一 IP 每分钟最多 10 次
pub async fn login_rate_limit(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let ip = addr.ip().to_string();
    let key = format!("rl:login:{}", ip);
    rate_limit_check(state.redis.clone(), &key, 10, 60, req, next).await
}

/// 公开 API 限流：同一 IP 每分钟最多 60 次
pub async fn api_rate_limit(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let ip = addr.ip().to_string();
    let key = format!("rl:api:{}", ip);
    rate_limit_check(state.redis.clone(), &key, 60, 60, req, next).await
}

/// 激活专用限流：同一 IP 每分钟最多 20 次激活请求（防黄牛批量激活）
/// 比通用 api_rate_limit 更严格，单独作用于 /v1/activate
pub async fn activate_rate_limit(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let ip = addr.ip().to_string();
    let key = format!("rl:activate:{}", ip);
    rate_limit_check(state.redis.clone(), &key, 20, 60, req, next).await
}

/// 通用限流实现
/// - key: Redis key
/// - max: 窗口内最大请求数
/// - window_secs: 窗口时长（秒）
async fn rate_limit_check(
    mut redis: redis::aio::ConnectionManager,
    key: &str,
    max: i64,
    window_secs: u64,
    req: Request<Body>,
    next: Next,
) -> Response {
    // INCR 原子自增，首次创建时设置过期时间
    let count: i64 = redis.incr(key, 1_i64).await.unwrap_or(0);
    if count == 1 {
        // 首次请求，设置过期
        let _: () = redis.expire(key, window_secs as i64).await.unwrap_or(());
    }

    if count > max {
        // 获取剩余过期时间
        let ttl: i64 = redis.ttl(key).await.unwrap_or(window_secs as i64);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_str(&ttl.to_string()).unwrap(),
            )],
            Json(json!({
                "success": false,
                "message": format!("请求过于频繁，请 {} 秒后重试", ttl)
            })),
        )
            .into_response();
    }

    next.run(req).await
}

