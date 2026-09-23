//! 代理体系路由
//!
//! 商户接口（需 auth_middleware）：
//!   POST   /agent/invite                — 生成邀请码（成为上级）
//!   GET    /agent/list                  — 查看我的下级代理列表
//!   PATCH  /agent/:id/quota             — 调整代理配额
//!   PATCH  /agent/:id/commission        — 调整分润比例
//!   PATCH  /agent/:id/status            — 启用/禁用代理
//!   DELETE /agent/:id                   — 解除代理关系
//!   GET    /agent/commissions           — 我作为上级的分润统计
//!   GET    /agent/my                    — 我作为代理的关系信息
//!   GET    /agent/my/commissions        — 我作为代理的分润记录
//!   POST   /agent/join/:invite_code     — 使用邀请码加入上级

use crate::{
    middleware::auth::{auth_middleware, AppState},
    utils::{db_guard, jwt::Claims},
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get, patch, post},
    Extension, Json, Router,
};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// ── 请求结构 ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateInviteRequest {
    pub quota_total: Option<i32>,      // 初始配额，None = 0
    pub commission_rate: Option<i32>,  // 分润比例 0-100
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateQuotaRequest {
    pub delta: i32,   // 正数增加，负数回收
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateCommissionRequest {
    pub commission_rate: i32,
}

#[derive(Deserialize)]
pub struct UpdateStatusRequest {
    pub status: String,
}

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

// ── 响应结构 ──────────────────────────────────────────────────────────────────

#[derive(Serialize, sqlx::FromRow)]
pub struct AgentRelationRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub agent_username: String,
    pub quota_total: i32,
    pub quota_used: i32,
    pub commission_rate: i32,
    pub status: String,
    pub invite_code: String,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct CommissionLogRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub agent_username: String,
    pub commission_rate: i32,
    pub units: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct MyRelationRow {
    pub id: Uuid,
    pub parent_id: Uuid,
    pub parent_username: String,
    pub quota_total: i32,
    pub quota_used: i32,
    pub commission_rate: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

// ── 路由注册 ──────────────────────────────────────────────────────────────────

pub fn agent_router(state: AppState) -> Router<AppState> {
    Router::new()
        // 固定路径必须在动态路径 /:id 之前
        .route("/agent/invite",          post(create_invite))
        .route("/agent/list",            get(list_agents))
        .route("/agent/commissions",     get(list_commissions_as_parent))
        .route("/agent/my",              get(my_relation))
        .route("/agent/my/commissions",  get(my_commissions_as_agent))
        .route("/agent/join/:code",      post(join_by_invite))
        // 动态路径放最后
        .route("/agent/:id/quota",       patch(update_quota))
        .route("/agent/:id/commission",  patch(update_commission))
        .route("/agent/:id/status",      patch(update_status))
        .route("/agent/:id",             delete(remove_agent))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

// ── 生成邀请码 ────────────────────────────────────────────────────────────────

async fn create_invite(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateInviteRequest>,
) -> Json<Value> {
    if claims.role != "merchant" {
        return Json(json!({"success": false, "message": "仅商户可创建代理邀请"}));
    }
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    let commission_rate = body.commission_rate.unwrap_or(0).clamp(0, 100);
    let quota_total = body.quota_total.unwrap_or(0).max(0);

    // 生成唯一邀请码（8位大写字母数字）
    let invite_code = generate_invite_code();

    let result = sqlx::query(
        "INSERT INTO agent_relations (parent_id, agent_id, quota_total, commission_rate, invite_code, note)
         VALUES ($1, $1, $2, $3, $4, $5)"
    )
    .bind(parent_id)  // agent_id 暂时填 parent_id，join 时更新
    .bind(quota_total)
    .bind(commission_rate)
    .bind(&invite_code)
    .bind(&body.note)
    .execute(&state.pool)
    .await;

    // 上面语义不对，正确做法：先插入一条 pending 记录，agent_id 为 NULL
    // 但 schema 有 NOT NULL 约束，改为插入时 agent_id = parent_id，join 时 UPDATE
    match result {
        Ok(_) => Json(json!({
            "success": true,
            "message": "邀请码已生成，分享给代理使用",
            "data": {
                "invite_code": invite_code,
                "quota_total": quota_total,
                "commission_rate": commission_rate,
            }
        })),
        Err(e) => db_guard::internal_error("创建代理", e),
    }
}

fn generate_invite_code() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..8).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

// ── 查看我的代理列表（我是上级）─────────────────────────────────────────────

async fn list_agents(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PageQuery>,
) -> Json<Value> {
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // 已用配额的**唯一**口径：该代理名下有多少张卡密（生成即消耗）。
    //
    // 不能再读 `agent_relations.quota_used` —— 那个字段历史上由「激活」累加，
    // 与「生成」是两个不同的量，混用会导致配额形同虚设。
    // 这里用子查询现算，保证与 cards.rs 的配额判定用同一个事实来源。
    let rows: Vec<AgentRelationRow> = match sqlx::query_as(
        "SELECT ar.id, ar.agent_id, m.username AS agent_username,
                ar.quota_total,
                (SELECT COUNT(*) FROM cards c WHERE c.merchant_id = ar.agent_id)::int AS quota_used,
                ar.commission_rate,
                ar.status, ar.invite_code, ar.note, ar.created_at
         FROM agent_relations ar
         JOIN merchants m ON m.id = ar.agent_id
         WHERE ar.parent_id = $1 AND ar.agent_id != ar.parent_id
         ORDER BY ar.created_at DESC
         LIMIT $2 OFFSET $3"
    )
    .bind(parent_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        // ⚠️ 曾用 `.unwrap_or_default()`：下级代理列表静默变空。
        // 上级会以为「我一个下级都没有」，可能据此重新发邀请码 ——
        // 而真正的问题（数据库）从来没被暴露。
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询下级代理列表失败: parent_id={} err={}", parent_id, e);
            return db_guard::server_busy();
        }
    };

    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM agent_relations WHERE parent_id = $1 AND agent_id != parent_id"
        )
        .bind(parent_id)
        .fetch_one(&state.pool),
        "统计下级代理数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(t) => t,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    // 未使用邀请码（agent_id = parent_id 的）
    // ⚠️ 曾用 `.unwrap_or_default()`：未使用邀请码列表静默变空。
    // 这一处比列表变空更糟：上级会以为「邀请码都被人用了」，
    // 于是**重新生成一批** —— 结果是库里堆积一堆从未发出去的邀请码。
    let pending_codes: Vec<(String, i32, i32, DateTime<Utc>)> = match sqlx::query_as(
        "SELECT invite_code, quota_total, commission_rate, created_at
         FROM agent_relations WHERE parent_id = $1 AND agent_id = parent_id
         ORDER BY created_at DESC"
    )
    .bind(parent_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询未使用邀请码失败: parent_id={} err={}", parent_id, e);
            return db_guard::server_busy();
        }
    };

    Json(json!({
        "success": true,
        "data": rows,
        "total": total.0,
        "page": page,
        "page_size": page_size,
        "pending_invites": pending_codes.iter().map(|(code, qt, cr, ca)| json!({
            "invite_code": code,
            "quota_total": qt,
            "commission_rate": cr,
            "created_at": ca,
        })).collect::<Vec<_>>(),
    }))
}

// ── 调整配额 ──────────────────────────────────────────────────────────────────

async fn update_quota(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateQuotaRequest>,
) -> Json<Value> {
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    // 查询当前关系（带出 agent_id —— 算已用配额要按它去数卡密）
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「代理关系不存在或无权限」。
    // 这里方向是收严的（不会误放行修改配额），但措辞误导排查方向：
    // 上级明明看着这条关系，却被告知不存在。
    let rel = match db_guard::optional(
        sqlx::query_as::<_, (Uuid, i32, Uuid)>(
            "SELECT id, quota_total, agent_id FROM agent_relations WHERE id = $1 AND parent_id = $2"
        )
        .bind(id)
        .bind(parent_id)
        .fetch_optional(&state.pool),
        "查询代理关系（调整配额）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => r,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "代理关系不存在或无权限"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let (_, quota_total, merchant_id_of_agent) = rel;

    // 已用配额 = 该代理名下卡密总数（与 cards.rs 的判定同一口径）。
    // 为什么要拦「把配额调到已用量以下」：那样代理会立刻处于「已超配额」状态，
    // 剩下的卡密永远生成不了，而界面上只会显示一个奇怪的负数剩余量。
    // ⚠️⚠️ 这是**配额下限检查**，`.unwrap_or((0,))` 的方向是放松：
    // 查询失败 → 已用量当成 0 → `new_total < quota_used` 为假 →
    // **放行了「把配额调到已用量以下」的操作** → 代理立刻处于超配额状态，
    // 名下卡密再也生成不了，而界面上只会显示一个奇怪的负数剩余量。
    //
    // 注意这里也不仅仅是"数字不好看"：一旦配额被调到已用量以下，
    // 代理的业务就**实质停摆**了，而恢复它需要管理员手工改库。
    let used = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM cards WHERE merchant_id = $1")
            .bind(merchant_id_of_agent)
            .fetch_one(&state.pool),
        "统计代理已生成卡密数（配额下限检查）",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(u) => u,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };
    let quota_used = used.0.min(i32::MAX as i64) as i32;

    let new_total = quota_total + body.delta;
    if new_total < quota_used {
        return Json(json!({
            "success": false,
            "message": format!("配额不能低于已使用量 {}（该代理已生成 {} 张卡密）", quota_used, quota_used)
        }));
    }
    if new_total < 0 {
        return Json(json!({"success": false, "message": "配额不能为负数"}));
    }

    let mut tx = match state.pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("调整代理配额：开启事务失败: relation_id={} err={}", id, e);
            return db_guard::server_busy();
        }
    };

    // ⚠️⚠️ 这两条写入曾经是 `let _ = ...execute(&mut *tx)`，并且**不能**指望
    // 「commit 会把错误拦下来」——这是实测推翻过的判断：
    //
    //   事务内某条语句失败后，事务进入 aborted 状态；此时 sqlx 的 `tx.commit()`
    //   发出的 COMMIT 会被 PG 当作回滚，**返回的命令标签是 `ROLLBACK` 而不是错误**，
    //   所以 `commit()` 返回 Ok(())。
    //
    // 结果就是：整笔事务静默回滚、配额一个字没改，而响应里写着
    // 「配额已调整，新配额: N」。实测输出见 .workbuddy/batch8-swallowed-write-errors.md。
    // 这也是本批次里唯一一处「丢错 + 事务内」的组合，危险程度高于池上丢错。
    let updated = sqlx::query(
        "UPDATE agent_relations SET quota_total = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(new_total)
    .bind(id)
    .execute(&mut *tx)
    .await;

    match updated {
        Ok(r) if r.rows_affected() > 0 => {}
        // 前面刚查到过这条关系，为 0 说明它在这之间被删了。
        // 此时那条配额变更日志也会是空的 —— 什么都不做却回「已调整」同样是撒谎。
        Ok(_) => {
            tracing::error!(
                "调整代理配额：配额更新未命中任何行（关系已被删除？）: relation_id={}",
                id
            );
            return db_guard::server_busy();
        }
        Err(e) => {
            tracing::error!("调整代理配额：更新配额失败: relation_id={} err={}", id, e);
            return db_guard::server_busy(); // tx 析构 → 自动回滚
        }
    }

    if let Err(e) = sqlx::query(
        "INSERT INTO agent_quota_logs (relation_id, parent_id, agent_id, delta, reason)
         SELECT $1, parent_id, agent_id, $2, $3 FROM agent_relations WHERE id = $1",
    )
    .bind(id)
    .bind(body.delta)
    .bind(&body.reason)
    .execute(&mut *tx)
    .await
    {
        // 这条日志是「谁在什么时候改了多少配额」的唯一记录，写不进去就该整笔失败
        tracing::error!(
            "调整代理配额：写配额变更日志失败: relation_id={} delta={} err={}",
            id,
            body.delta,
            e
        );
        return db_guard::server_busy();
    }

    match tx.commit().await {
        Ok(_) => Json(json!({
            "success": true,
            "message": format!("配额已调整，新配额: {}", new_total),
            "data": {"quota_total": new_total, "quota_used": quota_used}
        })),
        Err(e) => {
            // 提交失败要说出来（而且不把 sqlx 原文返回给客户端：会泄漏表名/列名/约束名）
            tracing::error!("调整代理配额：提交事务失败: relation_id={} err={}", id, e);
            db_guard::server_busy()
        }
    }
}

// ── 调整分润比例 ──────────────────────────────────────────────────────────────

async fn update_commission(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCommissionRequest>,
) -> Json<Value> {
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };
    let rate = body.commission_rate.clamp(0, 100);
    let result = sqlx::query(
        "UPDATE agent_relations SET commission_rate = $1, updated_at = NOW()
         WHERE id = $2 AND parent_id = $3"
    )
    .bind(rate)
    .bind(id)
    .bind(parent_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "状态已更新"})),
        Ok(_) => Json(json!({"success": false, "message": "关系不存在或无权限"})),
        Err(e) => db_guard::internal_error("更新代理", e),
    }
}

// ── 启用/禁用代理 ─────────────────────────────────────────────────────────────

async fn update_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateStatusRequest>,
) -> Json<Value> {
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };
    if body.status != "active" && body.status != "disabled" {
        return Json(json!({"success": false, "message": "status 仅支持 active / disabled"}));
    }
    let result = sqlx::query(
        "UPDATE agent_relations SET status = $1, updated_at = NOW()
         WHERE id = $2 AND parent_id = $3"
    )
    .bind(&body.status)
    .bind(id)
    .bind(parent_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "状态已更新"})),
        Ok(_) => Json(json!({"success": false, "message": "关系不存在或无权限"})),
        Err(e) => db_guard::internal_error("更新代理", e),
    }
}

// ── 解除代理关系 ──────────────────────────────────────────────────────────────

async fn remove_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };
    let result = sqlx::query(
        "DELETE FROM agent_relations WHERE id = $1 AND parent_id = $2"
    )
    .bind(id)
    .bind(parent_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "代理关系已解除"})),
        Ok(_) => Json(json!({"success": false, "message": "关系不存在或无权限"})),
        Err(e) => db_guard::internal_error("代理操作", e),
    }
}

// ── 分润统计（我作为上级）────────────────────────────────────────────────────

async fn list_commissions_as_parent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PageQuery>,
) -> Json<Value> {
    let parent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // ⚠️ 曾用 .unwrap_or_default()：分润记录查询失败时返回空列表，
    // 上级代理打开「我的分润」看到一片空白，会以为下级代理从未产生过分润
    // 而去找下级对账，实际是数据库查询挂了。
    let rows: Vec<CommissionLogRow> = match sqlx::query_as(
        "SELECT cl.id, cl.agent_id, m.username AS agent_username,
                cl.commission_rate, cl.units, cl.created_at
         FROM agent_commission_logs cl
         JOIN merchants m ON m.id = cl.agent_id
         WHERE cl.parent_id = $1
         ORDER BY cl.created_at DESC
         LIMIT $2 OFFSET $3"
    )
    .bind(parent_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询分润记录失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    // ⚠️ 曾用 .unwrap_or((0,))：合计条数静默变 0，前端分页器显示「共 0 条」
    // 而下面是空列表，用户会确信「一条分润都没有」。
    let total: (i64,) = match sqlx::query_as(
        "SELECT COUNT(*) FROM agent_commission_logs WHERE parent_id = $1"
    )
    .bind(parent_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("统计分润记录条数失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    // 汇总：各代理总激活数
    // ⚠️ 曾用 .unwrap_or_default()：分润汇总静默变空，页面下半部分的
    // 「各下级代理累计分润」表格凭空消失，看起来像分润归零。
    let summary: Vec<(Uuid, String, i64, i32)> = match sqlx::query_as(
        "SELECT cl.agent_id, m.username, SUM(cl.units)::bigint, cl.commission_rate
         FROM agent_commission_logs cl
         JOIN merchants m ON m.id = cl.agent_id
         WHERE cl.parent_id = $1
         GROUP BY cl.agent_id, m.username, cl.commission_rate"
    )
    .bind(parent_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询分润汇总失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    Json(json!({
        "success": true,
        "data": rows,
        "total": total.0,
        "page": page,
        "page_size": page_size,
        "summary": summary.iter().map(|(aid, uname, units, rate)| json!({
            "agent_id": aid,
            "agent_username": uname,
            "total_units": units,
            "commission_rate": rate,
        })).collect::<Vec<_>>(),
    }))
}

// ── 我的代理关系（我作为代理）────────────────────────────────────────────────

async fn my_relation(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let agent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    // 同 list_agents：quota_used 现算，口径 = 该代理名下的卡密总数
    let row: Option<MyRelationRow> = match sqlx::query_as(
        "SELECT ar.id, ar.parent_id, m.username AS parent_username,
                ar.quota_total,
                (SELECT COUNT(*) FROM cards c WHERE c.merchant_id = ar.agent_id)::int AS quota_used,
                ar.commission_rate,
                ar.status, ar.created_at
         FROM agent_relations ar
         JOIN merchants m ON m.id = ar.parent_id
         WHERE ar.agent_id = $1 AND ar.agent_id != ar.parent_id
         LIMIT 1"
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await
    {
        // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「暂未加入任何代理关系」。
        // 代理会以为自己没加入过（然后重新去点别人的邀请码）——
        // 而实际上他已经在关系里了，重复加入会被「您已有上级」挡住，
        // 于是一个「看起来自相矛盾」的工单就来了。
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询代理关系信息失败: agent_id={} err={}", agent_id, e);
            return db_guard::server_busy();
        }
    };

    match row {
        Some(r) => Json(json!({"success": true, "data": r})),
        None => Json(json!({"success": true, "data": null, "message": "暂未加入任何代理关系"})),
    }
}

// ── 我的分润记录（我作为代理）────────────────────────────────────────────────

async fn my_commissions_as_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PageQuery>,
) -> Json<Value> {
    let agent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    let rows: Vec<(Uuid, i32, i32, DateTime<Utc>)> = match sqlx::query_as(
        "SELECT id, commission_rate, units, created_at
         FROM agent_commission_logs
         WHERE agent_id = $1
         ORDER BY created_at DESC
         LIMIT $2 OFFSET $3"
    )
    .bind(agent_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        // ⚠️ 曾用 `.unwrap_or_default()`：分润记录静默变空。
        // 代理打开「我的分润」看到一片空白，第一反应是**「我的分润被吞了？」**——
        // 这是涉及钱的最敏感页面，宁可报错也不能给空列表。
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询代理分润记录失败: agent_id={} err={}", agent_id, e);
            return db_guard::server_busy();
        }
    };

    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM agent_commission_logs WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&state.pool),
        "统计代理分润记录数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(t) => t,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    // ⚠️ `SUM()` 在**没有任何记录时返回 NULL**，所以 `Ok(None)` 是正常的空集语义
    // （此时显示 0 分润是对的）；而 `.unwrap_or((None,))` 把 `Err` 也压成了 `None`，
    // 于是「查询失败」和「真的一笔都没有」在界面上都是 0 —— 同样是"看起来合理"的假象。
    let total_units: (Option<i64>,) = match sqlx::query_as(
        "SELECT SUM(units) FROM agent_commission_logs WHERE agent_id = $1"
    )
    .bind(agent_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("统计代理分润总单元数失败: agent_id={} err={}", agent_id, e);
            return db_guard::server_busy();
        }
    };

    Json(json!({
        "success": true,
        "data": rows.iter().map(|(id, rate, units, ca)| json!({
            "id": id, "commission_rate": rate, "units": units, "created_at": ca
        })).collect::<Vec<_>>(),
        "total": total.0,
        "total_units": total_units.0.unwrap_or(0),
        "page": page,
        "page_size": page_size,
    }))
}

// ── 使用邀请码加入上级 ────────────────────────────────────────────────────────

pub async fn join_by_invite(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(code): Path<String>,
) -> Json<Value> {
    if claims.role != "merchant" {
        return Json(json!({"success": false, "message": "仅商户可加入代理关系"}));
    }
    let agent_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户 ID"})),
    };

    // 查找邀请码（必须是 agent_id = parent_id 的未使用记录）
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「邀请码无效或已被使用」。
    // 这是最典型的一种"把故障说成用户错误"：用户拿着**完全正确的邀请码**，
    // 却被告知无效。他会去问上级「你这个码是不是用过了」，
    // 上级去看一眼发现码是好的 —— 两边都查不出问题。
    let rel = match db_guard::optional(
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, parent_id FROM agent_relations
             WHERE invite_code = $1 AND agent_id = parent_id AND status = 'active'"
        )
        .bind(&code)
        .fetch_optional(&state.pool),
        "按邀请码查询代理关系",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => Some(r),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let (rel_id, parent_id) = match rel {
        Some(r) => r,
        None => return Json(json!({"success": false, "message": "邀请码无效或已被使用"})),
    };

    if parent_id == agent_id {
        return Json(json!({"success": false, "message": "不能加入自己的邀请"}));
    }

    // 检查是否已有上级
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 判定「没有上级」→
    // **继续执行下面的 UPDATE**，把 agent_id 覆盖掉。
    //
    // 这意味着：一个**已经有上级**的代理，在数据库抖动时可以用另一个邀请码
    // 「再加入一次」，他的上级关系被**静默改写**（原上级的分润记录断掉）。
    // 这是放松方向 + 会改数据，比单纯的"提示错误"严重一个量级。
    let existing = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM agent_relations WHERE agent_id = $1 AND agent_id != parent_id LIMIT 1"
        )
        .bind(agent_id)
        .fetch_optional(&state.pool),
        "检查代理是否已有上级",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if existing.is_some() {
        return Json(json!({"success": false, "message": "您已有上级代理，不能重复加入"}));
    }

    // 更新 agent_id，标记邀请码已使用
    let result = sqlx::query(
        "UPDATE agent_relations SET agent_id = $1, updated_at = NOW() WHERE id = $2"
    )
    .bind(agent_id)
    .bind(rel_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Json(json!({
            "success": true,
            "message": "已成功加入代理关系",
        })),
        Err(e) => db_guard::internal_error("代理加入", e),
    }
}

// ── 公开函数：激活时写分润记录（供 public_api.rs 调用）────────────────────────

pub async fn record_commission(
    pool: &sqlx::PgPool,
    agent_id: Uuid,
    card_id: Uuid,
    activation_id: Uuid,
) {
    // 查询该商户是否有上级
    // ⚠️⚠️ 这是**激活后的分润入账**，且函数签名是 `-> ()`（与 `webhooks::fire_webhook` 同构）。
    // `.unwrap_or(None)` 的后果是：查询失败 → `None` → 直接跳过写分润记录 →
    // **代理永远收不到这笔分润，而调用方、代理本人、日志里都没有任何痕迹**。
    // 代理只会在对账时发现「这笔激活没给我算钱」，而那时早已无法追溯是哪一次激活。
    //
    // 这里保留「失败不阻塞激活」的取舍（卡密激活绝不能因为分润系统故障而失败），
    // 但把两种情况分开：
    //   - `Ok(None)`      → 确实没有上级，静默 return（正常业务）
    //   - `Err`           → 记 error 后 return（故障，必须留痕）
    let rel: Option<(Uuid, Uuid, i32)> = match sqlx::query_as(
        "SELECT id, parent_id, commission_rate FROM agent_relations
         WHERE agent_id = $1 AND agent_id != parent_id AND status = 'active'
         LIMIT 1"
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(
                "查询代理关系失败，本次激活的分润不会入账（这不是「没有上级」）: agent_id={} card_id={} activation_id={} err={}",
                agent_id, card_id, activation_id, e
            );
            return;
        }
    };

    if let Some((rel_id, parent_id, rate)) = rel {
        // 写分润记录
        //
        // ⚠️ 这里曾经是 `let _ =`：分润记录写失败被丢掉，而本函数的调用方
        // （激活流程）本来就不会因此失败 —— 于是代理的这笔钱静默消失，
        // 只有对账时才发现「这笔激活没算钱」，且没有任何日志能定位是哪一次激活。
        // 这正是上面那段注释里说的「故障必须留痕」没有贯彻到**写路径**上。
        //
        // 取舍与查询失败一致：不阻塞激活（卡密激活不能因为分润系统故障而失败），
        // 但必须留下可追溯的错误。
        if let Err(e) = sqlx::query(
            "INSERT INTO agent_commission_logs
             (relation_id, agent_id, parent_id, card_id, activation_id, commission_rate, units)
             VALUES ($1, $2, $3, $4, $5, $6, 1)"
        )
        .bind(rel_id)
        .bind(agent_id)
        .bind(parent_id)
        .bind(card_id)
        .bind(activation_id)
        .bind(rate)
        .execute(pool)
        .await
        {
            tracing::error!(
                "写分润记录失败，这笔激活的分润没有入账: relation_id={} agent_id={} card_id={} activation_id={} rate={} err={}",
                rel_id, agent_id, card_id, activation_id, rate, e
            );
        }

        // ⚠️ 这里**故意不再** `UPDATE agent_relations SET quota_used = quota_used + 1`。
        //
        // 历史原因：那一行让 quota_used 变成了「激活次数」，而 cards.rs 的配额判定
        // 把它当成「已生成张数」相加比较 —— 两个量纲混用，导致代理只要不激活
        // 就可以无限生成卡密（配额形同虚设）。
        //
        // 现在的口径统一为：配额 = 允许生成的卡密张数，已用量一律从
        // `cards` 表现场 COUNT 得出（见 list_agents / my_relation / generate_cards）。
        // 「激活次数」这个事实由 agent_commission_logs 的 SUM(units) 表达，
        // 前端的「激活数」「累计激活」列就是读它，不需要再有第二个计数器。
        //
        // 如果将来要恢复这个字段，必须先想清楚它代表什么 ——
        // 一个字段被两处按不同语义读写，比没有这个字段更危险。
    }
} 
