//! 管理端操作审计中间件。
//!
//! 挂在所有 `admin_only` 路由的**最内层**（在鉴权 / admin_only 之后），
//! 于是既能拿到 `Claims`（知道是谁做的），又能在 handler 跑完之后拿到状态码。
//!
//! ── 为什么是中间件而不是在每个 handler 里加一行 ─────────────────────────
//!
//! 管理端写操作有 12+ 个（商户套餐、状态、删除，OAuth 配置、支付渠道配置、
//! 套餐配置…）。逐个手写 `audit::record(...)` 的问题是：
//!
//! * 新加一个接口，**没人会记得加**，而这个漏洞不会报错、不会被测试发现；
//! * 加在 handler 里只能记录"handler 认为成功"的结果，中途 `return` 的失败分支
//!   要各自补一遍，漏一个就少一条。
//!
//! 中间件把这件事变成结构性的：**只要接口挂在管理端路由上，就自动留痕**。
//!
//! ── 只记写操作 ──────────────────────────────────────────────────────────
//!
//! GET 不记。管理端列表页一次刷新就是好几个 GET，全记会让这张表迅速膨胀，
//! 反而把真正要看的几行埋掉。审计要回答的是"谁改了什么"，不是"谁看了什么"。

use crate::{
    middleware::auth::AppState,
    utils::audit::{self, AuditEntry},
    utils::jwt::Claims,
};
use axum::{
    body::Body,
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;

/// 审计需要读取请求体来提取白名单字段，因此必须有上限：
/// 没有上限的话，一个 100MB 的 body 会被整份读进内存。
/// 管理端接口全是小 JSON，2MB 已经远超实际需要。
const MAX_AUDIT_BODY: usize = 2 * 1024 * 1024;

/// 审计写入的超时。审计是旁路，**不能**因为库慢把管理端操作卡住：
/// sqlx 的默认 pool acquire 超时是 30 秒，不加这一层的话，
/// 数据库抖动期间管理员点一次"续期"要等半分钟才返回。
const AUDIT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn admin_audit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    if !matches!(
        method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return next.run(req).await;
    }

    let path = req.uri().path().to_string();
    // 路由模板（/admin/merchants/:id/plan）用于聚合"做了什么动作"；
    // 取不到时退回实际路径，宁可少一点聚合能力，也不能丢这条记录。
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| path.clone());

    let claims = req.extensions().get::<Claims>().cloned();
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0)
        // 必须用 trusted_proxies 解析，不能用 TCP 对端地址：
        // 反代后面所有请求的对端都是网关 IP，审计日志会变成一列相同的地址。
        .map(|peer| state.trusted_proxies.resolve(req.headers(), peer));

    // ── 读取请求体并重建请求 ────────────────────────────────────────────
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_AUDIT_BODY).await {
        Ok(b) => b,
        Err(_) => {
            // 超限或读取失败。此刻 body 已经不可恢复，只能拒绝请求 ——
            // 放过去等于"跳过审计执行一次写操作"，那比拒绝更糟。
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"success": false, "message": "请求体过大"})),
            )
                .into_response();
        }
    };
    let detail = audit::summarize_body(&bytes);
    let req = Request::from_parts(parts, Body::from(bytes));

    let resp = next.run(req).await;
    let status = resp.status().as_u16();

    let entry = AuditEntry {
        admin_id: claims
            .as_ref()
            .and_then(|c| uuid::Uuid::parse_str(&c.sub).ok()),
        admin_email: claims.as_ref().map(|c| c.email.as_str()),
        method: method.as_str(),
        route: &route,
        path: &path,
        status,
        detail: detail.as_ref(),
        ip,
    };

    // 失败（4xx / 5xx）也要记：连续失败往往正是"有人在试探"或"误操作卡住了"的信号。
    match tokio::time::timeout(AUDIT_WRITE_TIMEOUT, audit::record(&state.pool, entry)).await {
        Ok(()) => {}
        Err(_) => {
            tracing::error!(
                "审计日志写入超时（操作本身已返回 {}): method={} route={} path={}",
                status,
                method,
                route,
                path
            );
        }
    }

    resp
}
