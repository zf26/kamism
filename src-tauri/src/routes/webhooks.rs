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

    // ── 签名密钥的取舍 ────────────────────────────────────────────────────────
    //
    // ⚠️⚠️ 这里曾经是**全仓库最隐蔽的一处静默失败**，务必读完再改。
    //
    // 前端 `Apps.tsx` 的契约是「留空 = 不修改密钥」：label 写「签名密钥（留空则自动
    // 生成）」，已存在时 placeholder 写「不修改请留空」，提交时 `secret: 空 || undefined`
    // → 字段直接从 JSON 里消失。**商户的主观预期是：我只改了回调地址，密钥没动。**
    //
    // 而旧实现是：
    //     let secret = body.secret.filter(...).unwrap_or_else(|| 随机生成);
    //     ... secret = CASE WHEN $4 = '' THEN app_webhooks.secret ELSE EXCLUDED.secret END
    //
    // `$4` 绑的是上面那个**生成之后**的变量，它因为 `unwrap_or_else` 保证非空，
    // **永远不等于 `''`** → `CASE WHEN $4 = ''` 是死分支 → 每次保存都写 EXCLUDED.secret。
    //
    // 已实机坐实（`.workbuddy/probe-b15-webhook-secret.sh`）：
    //   带 secret=S1 创建 → 库内=S1
    //   不带 secret 再保存（改 URL）→ 库内变成另一个随机值
    //   显式传 secret="" 再保存     → 又变成第三个随机值
    // 后果：商户的接收端拿自己的密钥验签，**从此每一笔事件都验签失败**，
    // 而服务端日志只打了 `status=200`（请求发出去了），商户界面上密钥是掩码
    // （`abcd****wxyz`）也看不出变化。两边都没有任何线索指向「密钥被换了」。
    //
    // 修法：把「是否提供了新密钥」这件事实**显式地查出来**，再交给纯函数决策。
    // 关键点是不能只依赖 SQL 的 `ON CONFLICT` —— 那里的 `EXCLUDED.secret` 是
    // 「新值」，表达不了「沿用旧值」这件事，除非再引入一个 CASE 分支，
    // 而那正是上面那个死分支的成因。所以这里先读一次现有密钥。
    //
    // 并发说明：两个并发的 PUT 可能都读到 `None` 而各自生成密钥，后写的赢。
    // 两者都是刚生成的随机值、都没有对外生效过，所以**无害**（不像「覆盖用户
    // 自己设定的密钥」那样会破坏契约）。不为这点引入行锁。
    let existing: Option<String> = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT secret FROM app_webhooks WHERE app_id = $1")
            .bind(app_id)
            .fetch_optional(&state.pool),
        "查询现有 Webhook 密钥（upsert_webhook）",
    )
    .await
    {
        db_guard::QueryOutcome::Found((s,)) => Some(s),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    // secret 列是 `VARCHAR(64)`。超长会让 INSERT 直接报错，而那时用户只会看到
    // 一句「服务器内部错误」—— 指不到「密钥太长了」这个真正原因。
    // 注意按**字符数**判（PG 的 VARCHAR(n) 就是字符数），不能用 `.len()`（字节数），
    // 否则填 30 个汉字的密钥会被误判为超长。
    if let Some(s) = normalize_provided(body.secret.as_deref()) {
        let n = s.chars().count();
        if n > 64 {
            return Json(json!({
                "success": false,
                "message": format!("签名密钥不能超过 64 个字符（当前 {}）", n)
            }));
        }
    }

    let (secret, generated_plaintext) = match plan_secret(body.secret.as_deref(), existing.as_deref()) {
        // 沿用库里已有的密钥：本次未提供新密钥，配置也确实存在
        SecretPlan::Keep(prev) => (prev, None),
        // 用调用方提供的密钥
        SecretPlan::Replace(s) => (s, None),
        // 首次创建且未提供 → 自动生成。这个明文**必须回传一次**，见下方响应处理
        SecretPlan::Generate => {
            let s = new_random_secret();
            (s.clone(), Some(s))
        }
    };

    let enabled = body.enabled.unwrap_or(true);
    let events = body.events.unwrap_or_else(|| vec!["activate".to_string(), "verify".to_string()]);

    // 验证 events
    for e in &events {
        if e != "activate" && e != "verify" {
            return Json(json!({ "success": false, "message": format!("不支持的事件类型：{}", e) }));
        }
    }

    // SQL 里刻意**不再有「要不要覆盖密钥」的判断**：`secret` 已经是上面纯函数
    // 算好的最终值（沿用旧值 / 用户给的值 / 新生成的），这里无条件写。
    // 之前那个 `CASE WHEN $4 = '' THEN app_webhooks.secret ELSE EXCLUDED.secret END`
    // 之所以是死分支，根因就是「决策被塞进了 SQL，而 SQL 看不到『用户有没有提供』」。
    let result = sqlx::query(
        "INSERT INTO app_webhooks (app_id, merchant_id, url, secret, enabled, events)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (app_id) DO UPDATE
         SET url = EXCLUDED.url,
             secret = EXCLUDED.secret,
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
        Ok(_) => {
            let mut resp = json!({ "success": true, "message": "Webhook 配置已保存" });
            if let Some(plaintext) = generated_plaintext {
                // ⚠️ 这是商户**唯一一次**能拿到这把密钥的机会：
                // `get_webhook` / `list_webhooks` 返回的都是 `mask_middle` 掩码
                // （前4 + **** + 后4），无法反推出完整密钥。
                // 不回传的话，「留空则自动生成」这个选项就是**不可用**的 ——
                // 商户手里没有密钥，收端自然无法验签，而他根本不知道为什么。
                resp["message"] = json!("Webhook 配置已保存，请立即复制签名密钥（仅显示这一次）");
                resp["data"] = json!({ "secret": plaintext, "generated": true });
                // 日志里**只记事件、绝不记明文**（日志长期留存，且会被采集）。
                tracing::info!(
                    "Webhook 首次配置，已自动生成签名密钥并在本次响应中一次性返回: app_id={}",
                    app_id
                );
            }
            Json(resp)
        }
        Err(e) => db_guard::internal_error("保存 Webhook", e),
    }
}

/// 签名密钥的取值决策。
///
/// 抽成纯函数的原因：这段逻辑曾经藏在 SQL 的 `CASE WHEN` 里，而 SQL 看不到
/// 「调用方到底有没有提供密钥」这件事，于是分支永远是死分支（见 `upsert_webhook`
/// 的注释）。放在这里之后，「留空 = 不修改」这条契约可以被单元测试直接钉住，
/// 不需要起数据库。
#[derive(Debug, PartialEq)]
enum SecretPlan {
    /// 沿用库里已有的密钥
    Keep(String),
    /// 用调用方本次提供的密钥
    Replace(String),
    /// 首次创建且未提供密钥 → 自动生成
    Generate,
}

/// 归一化调用方提供的密钥：`None` / 空串 / 纯空白 **一律视为「未提供」**。
///
/// 判断用 `trim()`、返回值**不做 trim** —— 这是刻意的：旧实现就是这个行为，
/// 而这里要修的是「把未提供误当成提供了」，不是「顺手改掉密钥的规范化规则」。
/// 商户真要填带空格的密钥，那是他的选择（且前后空格会参与 HMAC 计算）。
fn normalize_provided(raw: Option<&str>) -> Option<&str> {
    raw.filter(|s| !s.trim().is_empty())
}

fn plan_secret(raw_provided: Option<&str>, existing: Option<&str>) -> SecretPlan {
    match (normalize_provided(raw_provided), existing) {
        // 提供了密钥 → 用户就是要换（即使已有配置，也以用户的为准）
        (Some(s), _) => SecretPlan::Replace(s.to_string()),
        // 没提供 + 已有配置 → **保留原密钥**。这是本条契约的核心，也是修复前的 bug 点。
        (None, Some(prev)) => SecretPlan::Keep(prev.to_string()),
        // 没提供 + 没有配置 → 首次创建，生成
        (None, None) => SecretPlan::Generate,
    }
}

/// 自动生成签名密钥：32 个 hex 字符（16 字节随机）。
///
/// 用 `Uuid::new_v4().simple()` 而不是用户可控输入，且长度落在 `VARCHAR(64)` 内。
fn new_random_secret() -> String {
    format!("{}", Uuid::new_v4().simple())
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
        // ⚠️ 这里曾把 `Ok(r)` 一律记成 `info!`。但 `Ok` 只代表「HTTP 往返成功」——
        // 目标返回 500 / 404 / 403 时 **reqwest 依然给 `Ok`**。后果是：
        //   · 日志上确实有 `status=500` 这一行，但没人会去 `grep` 一条 info
        //   · `grep -i 'webhook.*error'` 一条都捞不到，看起来一切正常
        //   · 而本函数**没有重试**，所以那一笔通知是**永久丢失**的
        // 这正是本仓库最贵的那类 bug：不是崩溃，是安静地做错事。
        // 现在按 HTTP 语义分级 —— 只有 2xx 才算投递成功。
        match res {
            Ok(r) if r.status().is_success() => {
                tracing::info!("Webhook 投递成功: event={} url={} status={}", event, url, r.status());
            }
            Ok(r) => {
                // 非 2xx：请求到达了目标，但目标拒绝或内部报错。本次通知已经丢了。
                tracing::error!(
                    "Webhook 投递被目标拒绝，本次通知已丢失（当前实现无重试）: event={} url={} status={}",
                    event,
                    url,
                    r.status()
                );
            }
            Err(e) => {
                // 传输层失败（DNS / 连接被拒 / 超时）。同样是永久丢失，
                // 所以是 `error!` 而不是原来的 `warn!`。
                tracing::error!(
                    "Webhook 投递失败，本次通知已丢失（当前实现无重试）: event={} url={} err={}",
                    event,
                    url,
                    e
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    // ══════════════════════════════════════════════════════════════════════════
    // 「留空 = 不修改签名密钥」这条契约的守门测试
    //
    // 修复前的行为（已实机坐实，见 .workbuddy/probe-b15-webhook-secret.sh）：
    // 未提供密钥时每次保存都会写一个新随机值。根因是决策藏在 SQL 的
    // `CASE WHEN $4 = ''` 里，而 `$4` 绑的是生成后的变量 —— 那个分支永远不成立。
    //
    // 红版判据：把 `plan_secret` 改成「未提供就 Replace(新随机值)」，
    // 下面 `missing_secret_keeps_existing` / `empty_string_...` /
    // `whitespace_only_...` 三条必须变红。
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn provided_secret_replaces_existing() {
        // 用户主动填了密钥 → 就是以他为准，哪怕已有配置
        assert_eq!(
            plan_secret(Some("NEW"), Some("OLD")),
            SecretPlan::Replace("NEW".to_string())
        );
    }

    #[test]
    fn missing_secret_keeps_existing() {
        // ← 这条是本批的核心：字段整个不出现时，密钥必须原封不动
        assert_eq!(
            plan_secret(None, Some("OLD")),
            SecretPlan::Keep("OLD".to_string())
        );
    }

    #[test]
    fn empty_string_secret_keeps_existing() {
        // 前端 `secret: form.secret || undefined` 走不到这里，但 API 直连会
        assert_eq!(
            plan_secret(Some(""), Some("OLD")),
            SecretPlan::Keep("OLD".to_string())
        );
    }

    #[test]
    fn whitespace_only_secret_keeps_existing() {
        // 只打了几个空格也算「没填」—— 否则商户敲了个空格键就把自己的密钥换掉了
        assert_eq!(
            plan_secret(Some("   "), Some("OLD")),
            SecretPlan::Keep("OLD".to_string())
        );
        assert_eq!(
            plan_secret(Some(" \t\n "), Some("OLD")),
            SecretPlan::Keep("OLD".to_string())
        );
    }

    #[test]
    fn first_time_without_secret_generates() {
        assert_eq!(plan_secret(None, None), SecretPlan::Generate);
        assert_eq!(plan_secret(Some(""), None), SecretPlan::Generate);
        assert_eq!(plan_secret(Some("  \t "), None), SecretPlan::Generate);
    }

    #[test]
    fn provided_secret_is_not_trimmed() {
        // 与原行为一致：**判断**用 trim，**写入**不做 trim。
        // 前后空格会参与 HMAC 计算，擅自 trim 会让商户那边的签名对不上。
        assert_eq!(
            plan_secret(Some(" S "), Some("OLD")),
            SecretPlan::Replace(" S ".to_string())
        );
    }

    #[test]
    fn generated_secret_is_32_hex_chars_and_unique() {
        let a = new_random_secret();
        let b = new_random_secret();
        assert_eq!(a.len(), 32, "自动生成的密钥应为 32 个 hex 字符，实际: {}", a);
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit()),
            "自动生成的密钥应全是 hex 字符，实际: {}",
            a
        );
        // 两次生成必须不同 —— 否则「自动生成」等于给所有人同一把钥匙
        assert_ne!(a, b, "两次生成的密钥不应该相同");
    }

    #[test]
    fn generated_secret_fits_the_column() {
        // secret 列是 VARCHAR(64)（按字符数计）。生成值必须落在长度校验之内，
        // 否则返回给商户的明文会是一个永远写不进库的值。
        let s = new_random_secret();
        assert!(
            s.chars().count() <= 64,
            "自动生成的密钥必须能塞进 VARCHAR(64)，实际 {} 字符",
            s.chars().count()
        );
    }

    #[test]
    fn normalize_keeps_original_text_but_rejects_blank() {
        // 归一化的边界：只有「空白」被剔除，其余原样返回
        assert_eq!(normalize_provided(None), None);
        assert_eq!(normalize_provided(Some("")), None);
        assert_eq!(normalize_provided(Some("  ")), None);
        assert_eq!(normalize_provided(Some("x")), Some("x"));
        assert_eq!(normalize_provided(Some(" x ")), Some(" x "));
    }
}

