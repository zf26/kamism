use crate::{
    middleware::auth::{AppState, auth_middleware},
    utils::{db_guard, jwt::Claims, mask},
};
use axum::{
    extract::{Path, State},
    middleware,
    routing::{get},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct WebhookUpsertRequest {
    pub url: String,
    pub secret: Option<String>,
    pub enabled: Option<bool>,
    pub events: Option<Vec<String>>,
}

pub fn webhooks_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list_webhooks))
        .route("/webhooks/app/:app_id", get(get_webhook).put(upsert_webhook).delete(delete_webhook))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

/// 列出当前商户所有 Webhook 配置
async fn list_webhooks(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效的用户标识"})),
    };
    // ⚠️ 曾用 `.unwrap_or_default()`：查询失败 → 空列表 → 页面显示「还没配 Webhook」。
    // 用户据此以为配置丢了，会去重新配一遍（UPSERT 恰好能覆盖，所以看起来"修好了"），
    // 而真正的问题（数据库不可用）从来没被暴露出来。
    let rows: Vec<(Uuid, Uuid, String, String, bool, Vec<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        match sqlx::query_as(
            "SELECT id, app_id, url, secret, enabled, events, created_at, updated_at
             FROM app_webhooks WHERE merchant_id = $1 ORDER BY created_at DESC",
        )
        .bind(merchant_id)
        .fetch_all(&state.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询 Webhook 列表失败: merchant_id={} err={}", merchant_id, e);
                return db_guard::server_busy();
            }
        };

    let data: Vec<Value> = rows
        .into_iter()
        .map(|(id, app_id, url, secret, enabled, events, created_at, updated_at)| {
            json!({
                "id": id,
                "app_id": app_id,
                "url": url,
                "secret": mask::mask_middle(&secret),
                "enabled": enabled,
                "events": events,
                "created_at": created_at,
                "updated_at": updated_at,
            })
        })
        .collect();

    Json(json!({ "success": true, "data": data }))
}

/// 获取指定应用的 Webhook 配置
async fn get_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(app_id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效的用户标识"})),
    };
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「未配置 Webhook」。
    // 注意这里的矛盾感：用户明明配过，页面上却说未配置。频繁出现的「我的配置怎么没了」
    // 类工单，源头常常就是这一行。
    let row = match db_guard::optional(
        sqlx::query_as::<_, (Uuid, Uuid, String, String, bool, Vec<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
            "SELECT id, app_id, url, secret, enabled, events, created_at, updated_at
             FROM app_webhooks WHERE app_id = $1 AND merchant_id = $2",
        )
        .bind(app_id)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "查询单个 Webhook 配置",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => Some(r),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    match row {
        Some((id, app_id, url, secret, enabled, events, created_at, updated_at)) => Json(json!({
            "success": true,
            "data": {
                "id": id,
                "app_id": app_id,
                "url": url,
                "secret": mask::mask_middle(&secret),
                "enabled": enabled,
                "events": events,
                "created_at": created_at,
                "updated_at": updated_at,
            }
        })),
        None => Json(json!({ "success": false, "message": "未配置 Webhook" })),
    }
}

/// 创建或更新指定应用的 Webhook 配置（UPSERT）
async fn upsert_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(app_id): Path<Uuid>,
    Json(body): Json<WebhookUpsertRequest>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效的用户标识"})),
    };

    if body.url.trim().is_empty() {
        return Json(json!({ "success": false, "message": "URL 不能为空" }));
    }
    if !body.url.starts_with("http://") && !body.url.starts_with("https://") {
        return Json(json!({ "success": false, "message": "URL 必须以 http:// 或 https:// 开头" }));
    }

    // 验证 app 归属
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 返回「应用不存在或无权限」。
    // 这是收严方向（不会放行越权），但「无权限」这个措辞会把用户引向
    // 「我是不是选错了账号」的排查方向，而真实原因在数据库那侧。
    let app_exists = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM apps WHERE id = $1 AND merchant_id = $2")
            .bind(app_id)
            .bind(merchant_id)
            .fetch_optional(&state.pool),
        "校验应用归属（upsert_webhook）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if app_exists.is_none() {
        return Json(json!({ "success": false, "message": "应用不存在或无权限" }));
    }

    // 若未提供 secret 则生成随机 32 字节 hex 字符串
    let secret = body.secret
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            // 使用随机 UUID hex 作为默认 secret
            format!("{}", Uuid::new_v4().simple())
        });

    let enabled = body.enabled.unwrap_or(true);
    let events = body.events.unwrap_or_else(|| vec!["activate".to_string(), "verify".to_string()]);

    // 验证 events
    for e in &events {
        if e != "activate" && e != "verify" {
            return Json(json!({ "success": false, "message": format!("不支持的事件类型：{}", e) }));
        }
    }

    let result = sqlx::query(
        "INSERT INTO app_webhooks (app_id, merchant_id, url, secret, enabled, events)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (app_id) DO UPDATE
         SET url = EXCLUDED.url,
             secret = CASE WHEN $4 = '' THEN app_webhooks.secret ELSE EXCLUDED.secret END,
             enabled = EXCLUDED.enabled,
             events = EXCLUDED.events,
             updated_at = NOW()",
    )
    .bind(app_id)
    .bind(merchant_id)
    .bind(&body.url)
    .bind(&secret)
    .bind(enabled)
    .bind(&events)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Json(json!({ "success": true, "message": "Webhook 配置已保存" })),
        Err(e) => db_guard::internal_error("保存 Webhook", e),
    }
}

/// 删除指定应用的 Webhook 配置
async fn delete_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(app_id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效的用户标识"})),
    };
    let result = sqlx::query(
        "DELETE FROM app_webhooks WHERE app_id = $1 AND merchant_id = $2",
    )
    .bind(app_id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({ "success": true, "message": "Webhook 已删除" })),
        Ok(_) => Json(json!({ "success": false, "message": "Webhook 不存在或无权限" })),
        Err(e) => db_guard::internal_error("删除 Webhook", e),
    }
}

// 隐藏 secret 中间部分、只显示前 4 后 4 位，已抽到 `utils::mask::mask_middle`。
//
// ⚠️ 这里原先有个本地 `mask_secret`，写的是 `&s[..4]` / `&s[s.len() - 4..]` ——
// 两处都是**字节**索引，secret 含多字节字符时直接 panic
// （已实机坐实：`end byte index 4 is not a char boundary; it is inside '中'`）。
// 调用方可以自选 secret（`PUT /webhooks` 的 `secret` 字段），所以这是可达的 panic。
// 抽到公共模块的另一个原因：blacklist / public_api 有同样的字节切片，三处一起修。

/// 触发 Webhook 推送（由 public_api 调用，异步非阻塞）
pub async fn fire_webhook(
    pool: &sqlx::PgPool,
    app_id: Uuid,
    event: &str,
    payload: serde_json::Value,
) {
    let event = event.to_string(); // 转为 owned String，满足 tokio::spawn 的 'static 要求
    // 查询该应用是否有启用的 webhook 且包含该事件
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 直接 return → **通知从来没有发出去过，
    // 而调用方（`public_api`）完全无从知晓**。这是整个仓库里最难被发现的一类静默失败 ——
    // 没有报错、没有告警、没有任何界面提示，只是「通知就是不来的」。
    // 用户会在几天后才发现自己的业务系统接不到激活事件，而那时日志早已滚动过去了。
    //
    // 这里**故意保持**了「失败也不阻塞主流程」的取舍（webhook 是 best-effort，
    // 不能因为通知基础设施故障就让卡密激活失败），但把两件事分开了：
    //   - `NotFound` → `return`：确实没配 webhook，这是正常情况，静默是对的
    //   - `Err`      → `tracing::error!` 后 `return`：故障，必须留痕
    let row: Option<(String, String)> = match sqlx::query_as(
        "SELECT url, secret FROM app_webhooks
         WHERE app_id = $1 AND enabled = TRUE AND $2 = ANY(events)",
    )
    .bind(app_id)
    .bind(&event)
    .fetch_optional(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(
                "查询 Webhook 配置失败，本次事件不会推送（这不是「没有配置 Webhook」）: app_id={} event={} err={}",
                app_id,
                event,
                e
            );
            return;
        }
    };

    let (url, secret) = match row {
        Some(r) => r,
        None => return,
    };

    let timestamp = chrono::Utc::now().timestamp();
    let body = json!({
        "event": event,
        "timestamp": timestamp,
        "data": payload,
    })
    .to_string();

    // HMAC-SHA256 签名
    let signature = hmac_sha256_hex(&secret, &body);

    tokio::spawn(async move {
        // ⚠️ 曾用 `.unwrap_or_default()`。两个问题：
        //   1. 构建失败时 `Client::default()` **丢掉上面那个 10 秒超时** ——
        //      一个连不上的回调地址可能让这个连接永久挂着（不阻塞主流程，
        //      但会慢慢堆积，直到连接数耗尽）。
        //   2. 失败完全无痕。
        // 这里改成：失败就放弃本次投递，但明确记日志。
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("构建 HTTP 客户端失败，本次 Webhook 投递放弃: event={} err={}", event, e);
                return;
            }
        };
        let res = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-KamiSM-Event", event.clone())
            .header("X-KamiSM-Signature", format!("sha256={}", signature))
            .header("X-KamiSM-Timestamp", timestamp.to_string())
            .body(body)
            .send()
            .await;
        match res {
            Ok(r) => tracing::info!("Webhook {} -> {} status={}", event, url, r.status()),
            Err(e) => tracing::warn!("Webhook {} -> {} failed: {}", event, url, e),
        }
    });
}

fn hmac_sha256_hex(key: &str, data: &str) -> String {
    use sha2::{Sha256, Digest};
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<Sha256>;
    // 如果密钥太短（< 32 字节），先 SHA-256 哈希为固定长度
    let mac = if key.len() < 32 {
        let hashed = Sha256::digest(key.as_bytes());
        HmacSha256::new_from_slice(&hashed).expect("SHA-256 哈希后密钥长度应正好 32 字节")
    } else {
        HmacSha256::new_from_slice(key.as_bytes())
            .expect("密钥长度 >= 32 字节时应能正常构造 HMAC")
    };
    let mut mac = mac;
    mac.update(data.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

