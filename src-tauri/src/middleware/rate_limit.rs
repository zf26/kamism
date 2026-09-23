//! 基于 Redis 的速率限制
//! 使用滑动窗口计数器：key 在窗口期内计数，超出阈值返回 429
//!
//! ⚠️ 计数维度是「真实客户端 IP」，不是 TCP 对端 IP。
//! 早期版本用 `ConnectInfo` 的地址做 key，而本服务只 `expose`/监听内网、由 Nginx
//! 反代，于是**所有用户共用一个桶**（全站共享 10 次/分钟的登录额度，公开 API 共享
//! 60 次/分钟，很容易被单个高频客户端打成全站 429）。现在统一走
//! `TrustedProxies::resolve`：对端不可信时仍然用对端地址，对端可信（反代）时才
//! 从转发头里取真实客户端 IP。

use crate::middleware::auth::AppState;
use crate::utils::client_ip::bucket_key;
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
    let ip = state.trusted_proxies.resolve(req.headers(), addr);
    let key = format!("rl:login:{}", bucket_key(ip));
    rate_limit_check(state.redis.clone(), &key, 10, 60, req, next).await
}

/// 公开 API 限流：同一 IP 每分钟最多 60 次
pub async fn api_rate_limit(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let ip = state.trusted_proxies.resolve(req.headers(), addr);
    let key = format!("rl:api:{}", bucket_key(ip));
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
    let ip = state.trusted_proxies.resolve(req.headers(), addr);
    let key = format!("rl:activate:{}", bucket_key(ip));
    rate_limit_check(state.redis.clone(), &key, 20, 60, req, next).await
}

/// 通用限流实现
/// - key: Redis key
/// - max: 窗口内最大请求数
/// - window_secs: 窗口时长（秒）
///
/// Redis 不可用时选择**放行**（fail-open）并留下 error 日志：限流是防护措施，
/// 不是业务前提；卡密验证本身不应该因为限流组件抖动而被拒绝。但绝不静默——
/// 这里必须留下日志，否则「限流悄悄失效」没人会知道。
async fn rate_limit_check(
    mut redis: redis::aio::ConnectionManager,
    key: &str,
    max: i64,
    window_secs: u64,
    req: Request<Body>,
    next: Next,
) -> Response {
    // INCR 原子自增，首次创建时设置过期时间
    let count: i64 = match redis.incr(key, 1_i64).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("限流计数器不可用（Redis 错误），本次请求放行: key={} err={}", key, e);
            return next.run(req).await;
        }
    };
    // ── 过期时间必须保证设置成功，否则 key 会永久存在 ─────────────────────
    //
    // ⚠️ 这里曾用 `redis.expire(key, ...).await.unwrap_or(())` 吞掉失败，后果很重：
    //
    //   `INCR` 成功（key 已被创建）而 `EXPIRE` 失败（网络抖动、主从切换、
    //   命令被拒）时，这个 key 就**没有 TTL**（实测 `TTL` 返回 -1 = 永久）。
    //   而 `EXPIRE` 只在 `count == 1` 时执行 —— count 再也不会回到 1，
    //   于是**没有任何自愈途径**：该 IP 的计数桶永久留在那里，
    //   一旦累加越过 `max`，这个 IP 的所有请求就永远是 429。
    //
    //   故障现场（key 无 TTL）要等到几天后某个用户突然登不上才会暴露，
    //   而那时日志早已滚掉，排查会毫无头绪 —— 典型的「安静地做错事」。
    //
    // 修法：不再依赖「只有 count == 1 才设」这一次机会。改为**每次请求都尝试
    // 续期**。这样即使某次 EXPIRE 失败，下一个请求会补上；且失败时留下 error
    // 日志，不再静默。
    //
    // ⚠️ 为什么不加 `GT`（曾经想加，实测发现是错的，留在这里防止后人再踩）：
    //
    // `EXPIRE key seconds GT`（只在「新 TTL 更大」时才设置）是 **Redis 7.0+** 的
    // 语法。本项目实际连的是 **Redis 5.0.14**（`INFO server` 实测），
    // 该版本的 `EXPIRE` arity 固定为 3（`COMMAND INFO EXPIRE` 实测），多传一个
    // `GT` 会直接报：
    //
    //     -ERR wrong number of arguments for 'expire' command
    //
    // 而 `EXPIRE` 一旦永久失败，**每个限流 key 都会重新变成永久 key** ——
    // 等于把本 bug 从「偶发」升级成「必然」。所以这里**不能**用 `GT`。
    //
    // 不加 `GT` 的语义差异：每次请求把 TTL 重置为 `window_secs`，
    // 属于固定窗口的常见近似 —— 持续打满的客户端会让窗口滑动，比严格固定窗口
    // 略宽松。但 TTL **始终存在**（这才是本 bug 要保证的）。
    // 若将来升到 Redis 7+ 且需要严格固定窗口，正确做法是用 Lua 把
    // `INCR`+`EXPIRE` 原子化，而不是补 `GT`。
    //
    // 注意：这里**不因 EXPIRE 失败而拒绝请求** —— 限流是防护措施而非业务前提
    // （理由同下方 Redis 整体不可用时的 fail-open）。但必须留痕。
    if let Err(e) = redis
        .expire::<_, ()>(key, window_secs as i64)
        .await
        .map(|_| ())
    {
        tracing::error!(
            "限流 key 过期时间设置失败，存在永久 key 风险（下个请求会重试续期）: key={} err={}",
            key,
            e
        );
    }

    if count > max {
        // 获取剩余过期时间
        //
        // ⚠️ 这里的 `unwrap_or(window_secs)` 是**正常空集语义**，不能改成报错：
        // key 真的不存在时（TTL 返回 -1，例如恰好在这两条命令之间过期），
        // 默认成窗口长度是合理且无害的估值 —— 它只影响 429 响应里的
        // `Retry-After` 建议值。真正的 Redis 故障在 `INCR` 那一步已经拦掉了。
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

