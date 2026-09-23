//! 站内信 / 公告路由
//!
//! 管理员接口（需 admin_only）：
//!   POST   /admin/messages           — 发送公告或站内信
//!   GET    /admin/messages           — 查询已发消息列表
//!   PATCH  /admin/messages/:id       — 编辑消息（置顶/内容）
//!   DELETE /admin/messages/:id       — 删除消息
//!
//! 商户接口（需 auth_middleware）：
//!   GET     /merchant/notices          — 公告列表
//!   GET     /merchant/messages         — 站内信列表
//!   GET     /merchant/messages/unread_count — 未读数
//!   POST    /merchant/messages/:id/read    — 标记已读
//!   GET     /ws/messages               — WebSocket 升级

use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::message::{Message as Msg, MessageAdminView, MessageMerchantView},
    utils::{db_guard, jwt::Claims, ws::WsRegistry},
};
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    middleware,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Extension, Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// ── 路由注册 ─────────────────────────────────────────────────────────────────

pub fn messages_admin_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/messages", post(admin_send_message))
        .route("/admin/messages", get(admin_list_messages))
        .route("/admin/messages/:id", patch(admin_update_message))
        .route("/admin/messages/:id", delete(admin_delete_message))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

pub fn messages_merchant_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/merchant/notices", get(merchant_list_notices))
        .route("/merchant/messages", get(merchant_list_messages))
        .route("/merchant/messages/unread_count", get(merchant_unread_count))
        .route("/merchant/messages/:id/read", post(merchant_mark_read))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

/// WebSocket 路由。
///
/// ⚠️ 注意这里**没有**（也没办法）挂 `auth_middleware`：WS 升级依赖
/// `WebSocketUpgrade` 提取器，而 `middleware::from_fn` 拿到的是
/// `Request<Body>`，会把该提取器吃掉使路由匹配失败。
///
/// 因此**鉴权必须在 `ws_handler` 内部自己完成**（验签 + 版本校验 + 角色检查，
/// 三样都不能少）。`ws_handler` 上有详细注释说明为什么 ——
/// 早期版本只做了验签，导致令牌吊销机制被这条路径整个绕过。
///
/// 以后往这个 router 里加路由时，请同样在 handler 内部做完整鉴权。
pub fn messages_ws_router() -> Router<AppState> {
    Router::new().route("/ws/messages", get(ws_handler))
}

// ── 请求/响应结构 ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SendMessageRequest {
    /// 消息类型："notice" | "message"
    pub msg_type: String,
    pub title: String,
    pub content: String,
    /// 收件范围："all" | "single"（仅 message 类型有效）
    pub target_type: Option<String>,
    /// 单发时指定商户 UUID（优先级低于 target_email）
    pub target_id: Option<Uuid>,
    /// 单发时指定商户邮箱（优先于 target_id）
    pub target_email: Option<String>,
    pub pinned: Option<bool>,
    pub expires_at: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateMessageRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub pinned: Option<bool>,
    pub expires_at: Option<String>,
}

#[derive(Deserialize)]
pub struct MessageListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    /// 过滤类型："notice" | "message"，不传则全部
    pub msg_type: Option<String>,
}

// ── 管理员：发送消息 ──────────────────────────────────────────────────────────

async fn admin_send_message(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<SendMessageRequest>,
) -> Json<Value> {
    // 参数校验
    let msg_type = match body.msg_type.as_str() {
        "notice" | "message" => body.msg_type.clone(),
        _ => return Json(json!({"success": false, "message": "无效消息类型，仅支持 notice / message"})),
    };
    if body.title.trim().is_empty() {
        return Json(json!({"success": false, "message": "标题不能为空"}));
    }
    if body.content.trim().is_empty() {
        return Json(json!({"success": false, "message": "内容不能为空"}));
    }

    let sender_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效管理员 ID"})),
    };

    let target_type = if msg_type == "notice" {
        "all".to_string()
    } else {
        body.target_type.clone().unwrap_or_else(|| "all".to_string())
    };

    // single 类型：优先用 email 查找商户 id，其次用 target_id
    let resolved_target_id: Option<Uuid> = if target_type == "single" {
        if let Some(ref email) = body.target_email {
            // 按 email_hash 查找商户
            let email_hash = crate::db::encrypted_fields::EncryptedFieldsOps::generate_hash(email);
            let row = db_guard::optional(
                sqlx::query_as::<_, (Uuid,)>(
                    "SELECT id FROM merchants WHERE email_hash = $1 AND status = 'active'",
                )
                .bind(&email_hash)
                .fetch_optional(&state.pool),
                "查询收件商户邮箱",
            )
            .await;
            // ⚠️ 曾用 `.unwrap_or(None)`：数据库一抖 → 走到下面那条「未找到该邮箱对应的商户」。
            // 管理员明明在商户列表里看到这个邮箱，却被系统告知「没这个商户」——
            // 他会反复核对邮箱拼写、怀疑自己看错了列表，运维去查 merchants 表也查不出任何异常
            // （因为数据是好的），真正的问题（数据库/连接池）在日志里一个字都没有。
            match row {
                db_guard::QueryOutcome::Found((id,)) => Some(id),
                db_guard::QueryOutcome::NotFound => {
                    return Json(json!({"success": false, "message": "未找到该邮箱对应的商户"}))
                }
                db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
            }
        } else if let Some(id) = body.target_id {
            Some(id)
        } else {
            return Json(json!({"success": false, "message": "单发消息必须指定商户邮箱或 ID"}));
        }
    } else {
        None
    };

    let pinned = body.pinned.unwrap_or(false);
    let expires_at: Option<chrono::DateTime<chrono::Utc>> = body
        .expires_at
        .as_deref()
        .and_then(|s| s.parse().ok());

    let row: Result<(Uuid,), _> = sqlx::query_as(
        "INSERT INTO messages (type, title, content, sender_id, target_type, target_id, pinned, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(&msg_type)
    .bind(&body.title)
    .bind(&body.content)
    .bind(sender_id)
    .bind(&target_type)
    .bind(resolved_target_id)
    .bind(pinned)
    .bind(expires_at)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok((new_id,)) => {
            // 通过 WebSocket 实时推送
            let ws_payload = json!({
                "event": "new_message",
                "data": {
                    "id": new_id,
                    "type": msg_type,
                    "title": body.title,
                    "target_type": target_type,
                }
            })
            .to_string();

            let ws = state.ws_registry.clone();
            match target_type.as_str() {
                "all" => {
                    ws.broadcast(WsMessage::Text(ws_payload.into())).await;
                }
                "single" => {
                    if let Some(tid) = resolved_target_id {
                        ws.send_to(&tid, WsMessage::Text(ws_payload.into())).await;
                    }
                }
                _ => {}
            }

            Json(json!({"success": true, "message": "发送成功", "data": {"id": new_id}}))
        }
        Err(e) => db_guard::internal_error("发送消息", e),
    }
}

// ── 管理员：查询消息列表 ──────────────────────────────────────────────────────

async fn admin_list_messages(
    State(state): State<AppState>,
    Query(q): Query<MessageListQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let (total, rows): ((i64,), Vec<Msg>) = if let Some(ref t) = q.msg_type {
        // ⚠️ 曾用 `.unwrap_or((0,))`：查询失败 → total 变成 0。管理员看到「共 0 条」，
        // 会以为消息被删光了或自己发失败了，接着会去重新群发一遍 ——
        // 等数据库恢复，商户收到的是三份重复公告。分页组件也会显示 0 页，
        // 列表看起来真的像空的（rows 同样被降级成空），前后台互相佐证这个假象。
        let total = match db_guard::scalar(
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM messages WHERE type = $1")
                .bind(t)
                .fetch_one(&state.pool),
            "统计指定类型消息总数",
        )
        .await
        {
            db_guard::ScalarOutcome::Found(v) => v,
            db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
        };

        // ⚠️ 曾用 `.unwrap_or_default()`：和上面的 total 一起静默变空 ——
        // 「消息 0 条 + 列表为空」在界面上完全自洽，看不出是故障。
        let rows = match sqlx::query_as::<_, Msg>(
            "SELECT * FROM messages WHERE type = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(t)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询消息列表失败: type={} err={}", t, e);
                return db_guard::server_busy();
            }
        };
        (total, rows)
    } else {
        // ⚠️ 曾用 `.unwrap_or((0,))`：数据库故障时 total 静默变 0，
        // 而下面的 rows 也一起变成空列表 —— 于是「一条消息都没有」这个画面
        // 在数字和列表上完全自洽，看起来就是「后台确实没发过消息」，
        // 而不是「查不出来」。管理员的第一反应是去追责/重发，不是报故障。
        let total = match db_guard::scalar(
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM messages").fetch_one(&state.pool),
            "统计消息总数",
        )
        .await
        {
            db_guard::ScalarOutcome::Found(v) => v,
            db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
        };
        // ⚠️ 同上（全量分支）。
        let rows = match sqlx::query_as::<_, Msg>(
            "SELECT * FROM messages ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询全部消息列表失败: err={}", e);
                return db_guard::server_busy();
            }
        };
        (total, rows)
    };

    // 查询每条消息的已读数
    let views: Vec<MessageAdminView> = {
        let mut out = Vec::with_capacity(rows.len());
        for m in rows {
            // ⚠️ 曾用 `.unwrap_or((0,))`：查询失败 → 该条消息的已读数显示 0。
            // 管理员看着「已读 0 人」，会判定商户根本没看公告，于是改用短信/电话逐个通知 ——
            // 实际上商户早就在站内看过。对外的运营判断被一个坏掉的数字带偏了，
            // 而这个 0 长得和真实的 0 完全一样，事后无从分辨哪些数据是假的。
            let read_count = match db_guard::scalar(
                sqlx::query_as::<_, (i64,)>(
                    "SELECT COUNT(*) FROM message_reads WHERE message_id = $1",
                )
                .bind(m.id)
                .fetch_one(&state.pool),
                "统计消息已读数",
            )
            .await
            {
                db_guard::ScalarOutcome::Found(v) => v,
                db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
            };
            out.push(MessageAdminView {
                id: m.id,
                msg_type: m.msg_type,
                title: m.title,
                content: m.content,
                sender_id: m.sender_id,
                target_type: m.target_type,
                target_id: m.target_id,
                pinned: m.pinned,
                expires_at: m.expires_at,
                read_count: read_count.0,
                created_at: m.created_at,
            });
        }
        out
    };

    Json(json!({
        "success": true,
        "data": views,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

// ── 管理员：编辑消息 ──────────────────────────────────────────────────────────

async fn admin_update_message(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMessageRequest>,
) -> Json<Value> {
    // 检查消息是否存在
    // ⚠️ 曾用 `.unwrap_or(None)`：数据库故障 → exists 为 None → 返回「消息不存在」。
    // 管理员明明在列表里看着这条消息、URL 里的 id 也是从那条消息点进来的，
    // 编辑时却被告知「消息不存在」。他会去刷新页面、怀疑别人刚删了这条，
    // 而数据库里这条记录完好无损 —— 排查方向从一开始就是错的。
    let exists = db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM messages WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool),
        "校验消息是否存在",
    )
    .await;
    match exists {
        db_guard::QueryOutcome::Found(_) => {}
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "消息不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    }

    let expires_at: Option<chrono::DateTime<chrono::Utc>> = body
        .expires_at
        .as_deref()
        .and_then(|s| s.parse().ok());

    let result = sqlx::query(
        "UPDATE messages SET
            title      = COALESCE($1, title),
            content    = COALESCE($2, content),
            pinned     = COALESCE($3, pinned),
            expires_at = COALESCE($4, expires_at),
            updated_at = NOW()
         WHERE id = $5",
    )
    .bind(body.title)
    .bind(body.content)
    .bind(body.pinned)
    .bind(expires_at)
    .bind(id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Json(json!({"success": true, "message": "更新成功"})),
        Err(e) => db_guard::internal_error("更新消息", e),
    }
}

// ── 管理员：删除消息 ──────────────────────────────────────────────────────────

async fn admin_delete_message(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let result = sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "删除成功"})),
        Ok(_) => Json(json!({"success": false, "message": "消息不存在"})),
        Err(e) => db_guard::internal_error("删除消息", e),
    }
}

// ── 商户：公告列表 ────────────────────────────────────────────────────────────

async fn merchant_list_notices(
    State(state): State<AppState>,
    Query(q): Query<MessageListQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // ⚠️ 曾用 `.unwrap_or((0,))`：查询失败 → total 变 0，同时下面的 rows 也被降级成空，
    // 商户端「公告」页于是显示「暂无公告」并安静地停在那里。这比报错危险得多：
    // 平台方发了停机维护通知，商户看不到也不会察觉异常，到点直接撞上服务中断来找客服，
    // 而客服查后台又看到公告确实发出去了（数据是好的），双方各说各话。
    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM messages
             WHERE type = 'notice'
               AND (expires_at IS NULL OR expires_at > NOW())",
        )
        .fetch_one(&state.pool),
        "统计有效公告总数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(v) => v,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    let rows: Vec<Msg> = match sqlx::query_as(
        "SELECT * FROM messages
         WHERE type = 'notice'
           AND (expires_at IS NULL OR expires_at > NOW())
         ORDER BY pinned DESC, created_at DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        // ⚠️ 曾用 `.unwrap_or_default()`：商户的公告列表静默变空。
        // 商户看不到平台公告（比如「服务维护通知」「套餐规则变更」），
        // 而他**完全不知道有公告存在** —— 这类"没看到通知"的后果
        // 往往在几天后才以「我不知道有这回事」的形式暴露出来。
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询商户公告列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let views: Vec<MessageMerchantView> = rows
        .into_iter()
        .map(|m| MessageMerchantView {
            id: m.id,
            msg_type: m.msg_type,
            title: m.title,
            content: m.content,
            target_type: m.target_type,
            pinned: m.pinned,
            expires_at: m.expires_at,
            is_read: false, // 公告不追踪已读
            created_at: m.created_at,
        })
        .collect();

    Json(json!({
        "success": true,
        "data": views,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

// ── 商户：站内信列表 ──────────────────────────────────────────────────────────

async fn merchant_list_messages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<MessageListQuery>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // 查询：全体广播 + 发给自己的单发消息
    // ⚠️ 曾用 `.unwrap_or((0,))`：查询失败 → total 变 0，rows 也一起变空，
    // 商户端「站内信」页显示「共 0 条 / 暂无数据」。商户会得出结论
    // 「平台从来没给我发过消息」，而真相反而是查询本身就是坏的。
    // 由于未读数的降级方向恰好也是 0，两个接口会一起给出「一切正常」的假象。
    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM messages
             WHERE type = 'message'
               AND (target_type = 'all' OR (target_type = 'single' AND target_id = $1))",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool),
        "统计商户站内信总数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(v) => v,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    let rows: Vec<Msg> = match sqlx::query_as(
        "SELECT * FROM messages
         WHERE type = 'message'
           AND (target_type = 'all' OR (target_type = 'single' AND target_id = $1))
         ORDER BY created_at DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(merchant_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        // ⚠️ 曾用 `.unwrap_or_default()`：用户自己的消息列表静默变空。
        // 用户会以为「平台从没给我发过消息」，而实际上可能有未读的重要通知。
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询用户消息列表失败: merchant_id={} err={}", merchant_id, e);
            return db_guard::server_busy();
        }
    };

    // 批量查询已读状态
    let message_ids: Vec<Uuid> = rows.iter().map(|m| m.id).collect();
    let read_ids: Vec<(Uuid,)> = if message_ids.is_empty() {
        vec![]
    } else {
        // ⚠️ 这一处归到「可降级但必须留痕」类（本文件里唯一一处），
        // 与上面几处的处理**故意不同**，理由如下：
        //
        // 已读状态只影响列表上的「已读/未读」角标，不参与任何业务判定。
        // 查不出来时**全当未读**是安全方向（收严：不会把未读显示成已读，
        // 用户不会漏掉消息），代价只是角标不准。
        //
        // 而如果这里返回 503，就会因为一个角标问题让整个消息列表打不开 ——
        // 那比"角标不准"糟得多。
        //
        // 关键是**必须留痕**：用 `optional_lenient`（失败记 warn 后返回 None），
        // 而不是 `.unwrap_or_default()`（什么都不说）。
        db_guard::lenient_all(
            sqlx::query_as::<_, (Uuid,)>(
                "SELECT message_id FROM message_reads
                 WHERE merchant_id = $1 AND message_id = ANY($2)",
            )
            .bind(merchant_id)
            .bind(&message_ids)
            .fetch_all(&state.pool),
            "批量查询消息已读状态（角标，失败时按全未读处理）",
        )
        .await
    };
    let read_set: std::collections::HashSet<Uuid> = read_ids.into_iter().map(|(id,)| id).collect();

    let views: Vec<MessageMerchantView> = rows
        .into_iter()
        .map(|m| {
            let is_read = read_set.contains(&m.id);
            MessageMerchantView {
                id: m.id,
                msg_type: m.msg_type,
                title: m.title,
                content: m.content,
                target_type: m.target_type,
                pinned: m.pinned,
                expires_at: m.expires_at,
                is_read,
                created_at: m.created_at,
            }
        })
        .collect();

    Json(json!({
        "success": true,
        "data": views,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

// ── 商户：未读数 ──────────────────────────────────────────────────────────────

async fn merchant_unread_count(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    // ⚠️ 曾用 `.unwrap_or((0,))`：查询失败 → 未读数静默变成 0。
    // 这是整个文件里最刺眼的一处：商户端的小红点会清空，商户以为消息已经看完了，
    // 于是再也不会去点开站内信 —— 平台发的续费提醒、风控通知就这么躺在库里没人读。
    // 故障的表现是「一切已读」这个最让人安心的状态，没有任何一方会觉得需要报障，
    // 直到真的错过了截止时间。降级方向刚好指向「不需要处理」，这是最坏的方向。
    let count = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM messages
             WHERE type = 'message'
               AND (target_type = 'all' OR (target_type = 'single' AND target_id = $1))
               AND id NOT IN (
                   SELECT message_id FROM message_reads WHERE merchant_id = $1
               )",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool),
        "统计商户未读消息数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(v) => v,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    Json(json!({"success": true, "data": {"unread": count.0}}))
}

// ── 商户：标记已读 ────────────────────────────────────────────────────────────

async fn merchant_mark_read(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    // 幂等 upsert：重复标记不报错
    let result = sqlx::query(
        "INSERT INTO message_reads (message_id, merchant_id)
         VALUES ($1, $2)
         ON CONFLICT (message_id, merchant_id) DO NOTHING",
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Json(json!({"success": true, "message": "已标记已读"})),
        Err(e) => db_guard::internal_error("消息操作", e),
    }
}

// ── WebSocket 升级端点 ────────────────────────────────────────────────────────
//
// 连接方式：ws://host /ws/messages?token=<JWT>
// 前端在 query string 传入 access token，后端验证后注册连接

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    // 从 query string 取 token 并验证
    let token = params.get("token").cloned().unwrap_or_default();
    let claims = match crate::utils::jwt::verify_token(&token, &state.jwt_secret) {
        Ok(c) => c,
        Err(_) => {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "无效或过期的 Token",
            )
                .into_response();
        }
    };

    // 仅商户角色允许接入 WS
    if claims.role != "merchant" {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "仅商户可连接消息 WebSocket",
        )
            .into_response();
    }

    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "无效用户 ID",
            )
                .into_response();
        }
    };

    // ── 令牌版本（吊销）校验：这里曾经漏掉，是个安全旁路 ────────────────────
    //
    // ⚠️ 这个 handler **不走 `auth_middleware`**（WebSocket 升级需要 `WebSocketUpgrade`
    // 提取器，而 `middleware::from_fn` 的 `Request<Body>` 会把它吃掉，
    // 所以 router 层没办法复用那套中间件）。于是 `auth_middleware` 里的
    // 版本校验也一并漏了 —— 只做了 `verify_token`（验签）。
    //
    // 后果是**批次 4 刚建立的令牌吊销机制被整条绕过**：
    // 签名的有效性是「2 小时内」的事，吊销的有效性靠的是版本号比对。
    // 只验签意味着**改密、封号、重置 Key 都踢不掉已持有的旧 token** ——
    // 只要攻击者拿旧 token 连 WS，就能继续收实时推送（含公告、站内信）。
    //
    // 实测（同一个旧 token）：
    //   旧 token 调 /merchant/messages → 200
    //   改密码 → 旧 token 再调 /merchant/messages → 401   ← 普通接口是对的
    //   旧 token 连 /ws/messages → 101 Switching Protocols ← 漏洞
    //
    // 修复方向与 `auth_middleware` 保持一致，且**不做任何简化** —
    // `VersionCheck` 三态必须逐个处理，理由见那个枚举的注释。
    match crate::utils::jwt::check_token_version(
        &state.pool,
        &mut state.redis.clone(),
        &claims.role,
        &merchant_id,
        claims.ver,
    )
    .await
    {
        crate::utils::jwt::VersionCheck::Valid => {}
        crate::utils::jwt::VersionCheck::Revoked { token_ver, current_ver } => {
            tracing::warn!(
                "WebSocket 连接被拒：令牌已吊销: merchant_id={} token_ver={} current_ver={}",
                merchant_id,
                token_ver,
                current_ver
            );
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "登录状态已失效，请重新登录",
            )
                .into_response();
        }
        crate::utils::jwt::VersionCheck::Unavailable(e) => {
            // 与 `auth_middleware` 同样选 fail-closed：无法判定授权的连接
            // 不能被当成「已授权」。否则「把 Redis 搞挂」就成了绕过吊销的手法。
            // 注意这里**必须 `error!`** —— 静默拒绝会让线上出现
            // 「WebSocket 突然全连不上」而没人知道原因。
            tracing::error!("WebSocket 令牌版本校验无法完成，拒绝连接（fail-closed）: {}", e);
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "服务暂时不可用，请稍后重试",
            )
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_ws(socket, merchant_id, state.ws_registry))
        .into_response()
}

/// 处理单个 WebSocket 连接的生命周期
async fn handle_ws(socket: WebSocket, merchant_id: Uuid, registry: WsRegistry) {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let (ws_tx, mut ws_rx) = socket.split();
    // 用 Arc<Mutex> 包裹 sink，使两个 task 均可访问
    let ws_tx = Arc::new(Mutex::new(ws_tx));

    // 注册连接，获取内部消息接收端 + 本连接的发送端句柄
    let (mut msg_rx, my_sender) = registry.register(merchant_id).await;

    // 发送在线确认帧。
    //
    // ⚠️ 这里失败要**先注销再返回**：早期版本直接 `return`，于是这个连接
    // 已经写进注册表、却没有任何 task 在消费它，就永久留在 map 里
    // （且因为当时 channel 是 unbounded，`cleanup_dead` 也不会清它）。
    let hello = serde_json::json!({"event": "connected", "merchant_id": merchant_id}).to_string();
    if ws_tx
        .lock()
        .await
        .send(WsMessage::Text(hello.into()))
        .await
        .is_err()
    {
        registry.unregister(merchant_id, &my_sender).await;
        return;
    }

    let tx_a = ws_tx.clone();
    let task_a = async move {
        while let Some(msg) = msg_rx.recv().await {
            if tx_a.lock().await.send(msg).await.is_err() {
                break;
            }
        }
    };

    let tx_b = ws_tx.clone();
    let task_b = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                WsMessage::Close(_) => break,
                WsMessage::Ping(data) => {
                    let _ = tx_b.lock().await.send(WsMessage::Pong(data)).await;
                }
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = task_a => {}
        _ = task_b => {}
    }

    // ── 显式注销本连接 ──────────────────────────────────────────────────
    //
    // 精确摘掉**这一个** sender（同一商户可能有多个标签页，不能整体删）。
    // 过去这里只调 `cleanup_dead_pub`，它按 `is_closed()` 扫 —— 而当时
    // channel 是 unbounded，sender 从不「closed」，于是什么都没清掉。
    registry.unregister(merchant_id, &my_sender).await;
    // 兜底：顺手清掉同商户其他已断开的连接
    registry.cleanup_dead_pub(merchant_id).await;
}

