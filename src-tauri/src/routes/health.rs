//! GET /health — 依赖健康检查
//!
//! 返回 DB / Redis / MQ 三个依赖的连通性状态。
//! 无需鉴权，供监控系统（Uptime Robot、Prometheus 等）直接轮询。
//!
//! 响应示例（全部健康）：
//! ```json
//! { "status": "ok", "db": "ok", "redis": "ok", "mq": "ok", "uptime_secs": 3721 }
//! ```
//! 任意依赖不健康时 HTTP 状态码返回 503，status 字段为 "degraded"。

use crate::middleware::auth::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::sync::OnceLock;
use std::time::Instant;

/// 记录服务器启动时间，用于计算 uptime
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// 在 `start_server` 最早期调用，初始化启动时间戳
pub fn init_start_time() {
    START_TIME.get_or_init(Instant::now);
}

pub fn health_router() -> Router<AppState> {
    Router::new().route("/health", get(health_handler))
}

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = START_TIME
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);

    // ── DB ──────────────────────────────────────────────────────────────
    let db_ok = sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .is_ok();

    // ── Redis ────────────────────────────────────────────────────────────
    let redis_ok = {
        let mut conn = state.redis.clone();
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map(|r| r == "PONG")
            .unwrap_or(false)
    };

    // ── RabbitMQ ─────────────────────────────────────────────────────────
    // ChannelStatus::connected() 返回 bool，表示 channel 当前是否处于 Connected 状态
    let mq_ok = state.mq_channel.status().connected();

    let all_ok = db_ok && redis_ok && mq_ok;
    let status_str = if all_ok { "ok" } else { "degraded" };
    let http_status = if all_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };

    let body = json!({
        "status":      status_str,
        "db":          if db_ok    { "ok" } else { "error" },
        "redis":       if redis_ok { "ok" } else { "error" },
        "mq":          if mq_ok    { "ok" } else { "error" },
        "uptime_secs": uptime,
    });

    (http_status, Json(body))
}

