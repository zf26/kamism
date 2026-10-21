//! 管理员操作审计日志的写入与摘要。
//!
//! 记录动作本身由中间件 [`crate::middleware::audit::admin_audit`] 统一完成，
//! 它挂在**所有** `admin_only` 路由上 —— 这样"新加一个管理端接口忘了写审计"
//! 这种漏，在结构上就不可能发生（而不是靠每个 handler 自觉）。
//!
//! 本模块只负责两件容易写错的事：
//! 1. **[`record`]**：写入失败只记日志、绝不向上传播。审计是旁路，
//!    不能因为"日志表写不进去"就让管理员的正常操作失败 —— 那样只会让人
//!    把这张表摘掉，最后一条都不剩。
//! 2. **[`summarize_body`]**：从请求体里挑出**可记录的字段**。这里必须是**白名单**
//!    而不是黑名单：请求体里有 `client_secret`、支付私钥、OAuth 密钥，
//!    黑名单漏一个字段，审计日志自己就变成了新的凭据泄露源。

use serde_json::{Map, Value};
use std::net::IpAddr;
use uuid::Uuid;

/// 允许进审计日志的请求体字段。
///
/// 只放**运营语义**字段（改成了什么套餐 / 多少天 / 什么状态 / 开还是关）。
/// 任何看起来像凭据、密钥、令牌的字段都不在这里 —— 也就不可能被写进库。
const DETAIL_WHITELIST: &[&str] = &[
    "plan",
    "expires_days",
    "days",
    "status",
    "enabled",
    "provider",
    "channel",
    "name",
    "duration_days",
    "price",
];

/// 从请求体里提取白名单字段。
///
/// 返回 `None` 表示**没有任何可记录的内容**（非 JSON、不是对象、或白名单字段全不在），
/// 此时 `detail` 列写 NULL，而不是写一个 `{}` —— 前者能区分"没东西可记"
/// 和"记了但为空"，后者不能。
pub fn summarize_body(bytes: &[u8]) -> Option<Value> {
    let parsed: Value = serde_json::from_slice(bytes).ok()?;
    let obj = parsed.as_object()?;

    let mut picked = Map::new();
    for key in DETAIL_WHITELIST {
        if let Some(v) = obj.get(*key) {
            // 只保留标量 / 短数组，避免把大对象（如整份配置）塞进日志
            match v {
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                    picked.insert((*key).to_string(), v.clone());
                }
                Value::Array(items) if items.len() <= 16 && items.iter().all(|i| i.is_string()) => {
                    picked.insert((*key).to_string(), v.clone());
                }
                _ => {}
            }
        }
    }

    if picked.is_empty() {
        None
    } else {
        Some(Value::Object(picked))
    }
}

/// 一条待写入的审计记录。
pub struct AuditEntry<'a> {
    pub admin_id: Option<Uuid>,
    pub admin_email: Option<&'a str>,
    pub method: &'a str,
    /// 路由模板，如 `/admin/merchants/:id/plan`
    pub route: &'a str,
    /// 实际路径，含具体 id
    pub path: &'a str,
    pub status: u16,
    pub detail: Option<&'a Value>,
    pub ip: Option<IpAddr>,
}

/// 写入一条审计记录。
///
/// **失败只记日志**：审计表写不进去（磁盘满、表被误删）不应该让"给商户续期"
/// 这类正常操作失败。代价是这种情况下会出现"操作成功但不留痕"，
/// 所以失败日志用 `error!` 并且把整条记录打出来 —— 至少 stderr 里还有一份。
pub async fn record(pool: &sqlx::PgPool, entry: AuditEntry<'_>) {
    let ip_text = entry.ip.map(|ip| ip.to_string());
    let res = sqlx::query(
        "INSERT INTO admin_audit_logs
             (admin_id, admin_email, method, route, path, status, detail, ip)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(entry.admin_id)
    .bind(entry.admin_email)
    .bind(entry.method)
    .bind(entry.route)
    .bind(entry.path)
    // status 是 HTTP 状态码，100~599，放进 SMALLINT 不会溢出
    .bind(entry.status as i16)
    .bind(entry.detail)
    .bind(ip_text)
    .execute(pool)
    .await;

    if let Err(e) = res {
        tracing::error!(
            "审计日志写入失败（本次操作已生效，但库里没有留痕）: method={} route={} path={} status={} admin_id={:?} err={}",
            entry.method,
            entry.route,
            entry.path,
            entry.status,
            entry.admin_id,
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_operational_fields() {
        let body = br#"{"plan":"pro","expires_days":30,"status":"active"}"#;
        let got = summarize_body(body).expect("应提取到字段");
        assert_eq!(got["plan"], "pro");
        assert_eq!(got["expires_days"], 30);
        assert_eq!(got["status"], "active");
    }

    /// 这条是**安全底线**：密钥类字段绝不能进审计日志。
    #[test]
    fn never_keeps_secrets() {
        let body = br#"{
            "provider":"github",
            "client_secret":"super-secret-value",
            "client_id":"abc",
            "api_key":"k-1234567890",
            "password":"p@ssw0rd",
            "private_key":"-----BEGIN PRIVATE KEY-----",
            "enabled":true
        }"#;
        let text = summarize_body(body).expect("应有 provider/enabled").to_string();
        for secret in ["super-secret-value", "k-1234567890", "p@ssw0rd", "PRIVATE KEY", "abc"] {
            assert!(!text.contains(secret), "审计摘要泄露了 {secret}: {text}");
        }
        // 只留下白名单里的两个
        assert!(text.contains("github"));
        assert!(text.contains("true"));
    }

    #[test]
    fn returns_none_when_nothing_worth_recording() {
        assert!(summarize_body(b"not json").is_none());
        assert!(summarize_body(b"{}").is_none());
        // 只有非白名单字段
        assert!(summarize_body(br#"{"client_secret":"x"}"#).is_none());
        // 顶层是数组而不是对象
        assert!(summarize_body(br#"["plan"]"#).is_none());
    }

    #[test]
    fn drops_oversized_arrays_but_keeps_small_ones() {
        let small = br#"{"name":"basic"}"#;
        assert_eq!(summarize_body(small).unwrap()["name"], "basic");

        // 长数组不是"运营语义"，不进日志（防止把整份配置写进来）
        let big: String = format!(
            r#"{{"name":["{}"]}}"#,
            (0..20).map(|i| i.to_string()).collect::<Vec<_>>().join(r#"",""#)
        );
        assert!(summarize_body(big.as_bytes()).is_none());
    }
}
