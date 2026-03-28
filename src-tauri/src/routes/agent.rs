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
    utils::jwt::Claims,
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
        Err(e) => Json(json!({"success": false, "message": format!("创建失败: {}", e)})),
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

    let rows: Vec<AgentRelationRow> = sqlx::query_as(
        "SELECT ar.id, ar.agent_id, m.username AS agent_username,
                ar.quota_total, ar.quota_used, ar.commission_rate,
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
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM agent_relations WHERE parent_id = $1 AND agent_id != parent_id"
    )
    .bind(parent_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    // 未使用邀请码（agent_id = parent_id 的）
    let pending_codes: Vec<(String, i32, i32, DateTime<Utc>)> = sqlx::query_as(
        "SELECT invite_code, quota_total, commission_rate, created_at
         FROM agent_relations WHERE parent_id = $1 AND agent_id = parent_id
         ORDER BY created_at DESC"
    )
    .bind(parent_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

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

    // 查询当前关系
    let rel: Option<(Uuid, i32, i32)> = sqlx::query_as(
        "SELECT id, quota_total, quota_used FROM agent_relations WHERE id = $1 AND parent_id = $2"
    )
    .bind(id)
    .bind(parent_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (_, quota_total, quota_used) = match rel {
        Some(r) => r,
        None => return Json(json!({"success": false, "message": "代理关系不存在或无权限"})),
    };

    let new_total = quota_total + body.delta;
    if new_total < quota_used {
        return Json(json!({
            "success": false,
            "message": format!("配额不能低于已使用量 {}，当前已用 {}", quota_used, quota_used)
        }));
    }
    if new_total < 0 {
        return Json(json!({"success": false, "message": "配额不能为负数"}));
    }

    let mut tx = match state.pool.begin().await {
        Ok(t) => t,
        Err(e) => return Json(json!({"success": false, "message": format!("事务失败: {}", e)})),
    };

    let _ = sqlx::query(
        "UPDATE agent_relations SET quota_total = $1, updated_at = NOW() WHERE id = $2"
    )
    .bind(new_total)
    .bind(id)
    .execute(&mut *tx)
    .await;

    let _ = sqlx::query(
        "INSERT INTO agent_quota_logs (relation_id, parent_id, agent_id, delta, reason)
         SELECT $1, parent_id, agent_id, $2, $3 FROM agent_relations WHERE id = $1"
    )
    .bind(id)
    .bind(body.delta)
    .bind(&body.reason)
    .execute(&mut *tx)
    .await;

    match tx.commit().await {
        Ok(_) => Json(json!({
            "success": true,
            "message": format!("配额已调整，新配额: {}", new_total),
            "data": {"quota_total": new_total, "quota_used": quota_used}
        })),
        Err(e) => Json(json!({"success": false, "message": format!("提交失败: {}", e)})),
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
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
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
        Err(e) => Json(json!({"success": false, "message": format!("更新失败: {}", e)})),
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
        Err(e) => Json(json!({"success": false, "message": format!("操作失败: {}", e)})),
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

    let rows: Vec<CommissionLogRow> = sqlx::query_as(
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
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM agent_commission_logs WHERE parent_id = $1"
    )
    .bind(parent_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    // 汇总：各代理总激活数
    let summary: Vec<(Uuid, String, i64, i32)> = sqlx::query_as(
        "SELECT cl.agent_id, m.username, SUM(cl.units)::bigint, cl.commission_rate
         FROM agent_commission_logs cl
         JOIN merchants m ON m.id = cl.agent_id
         WHERE cl.parent_id = $1
         GROUP BY cl.agent_id, m.username, cl.commission_rate"
    )
    .bind(parent_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

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

    let row: Option<MyRelationRow> = sqlx::query_as(
        "SELECT ar.id, ar.parent_id, m.username AS parent_username,
                ar.quota_total, ar.quota_used, ar.commission_rate,
                ar.status, ar.created_at
         FROM agent_relations ar
         JOIN merchants m ON m.id = ar.parent_id
         WHERE ar.agent_id = $1 AND ar.agent_id != ar.parent_id
         LIMIT 1"
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

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

    let rows: Vec<(Uuid, i32, i32, DateTime<Utc>)> = sqlx::query_as(
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
    .unwrap_or_default();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM agent_commission_logs WHERE agent_id = $1"
    )
    .bind(agent_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    let total_units: (Option<i64>,) = sqlx::query_as(
        "SELECT SUM(units) FROM agent_commission_logs WHERE agent_id = $1"
    )
    .bind(agent_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((None,));

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
    let rel: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, parent_id FROM agent_relations
         WHERE invite_code = $1 AND agent_id = parent_id AND status = 'active'"
    )
    .bind(&code)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (rel_id, parent_id) = match rel {
        Some(r) => r,
        None => return Json(json!({"success": false, "message": "邀请码无效或已被使用"})),
    };

    if parent_id == agent_id {
        return Json(json!({"success": false, "message": "不能加入自己的邀请"}));
    }

    // 检查是否已有上级
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM agent_relations WHERE agent_id = $1 AND agent_id != parent_id LIMIT 1"
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

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
        Err(e) => Json(json!({"success": false, "message": format!("加入失败: {}", e)})),
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
    let rel: Option<(Uuid, Uuid, i32)> = sqlx::query_as(
        "SELECT id, parent_id, commission_rate FROM agent_relations
         WHERE agent_id = $1 AND agent_id != parent_id AND status = 'active'
         LIMIT 1"
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    if let Some((rel_id, parent_id, rate)) = rel {
        // 写分润记录
        let _ = sqlx::query(
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
        .await;

        // 更新已使用配额
        let _ = sqlx::query(
            "UPDATE agent_relations SET quota_used = quota_used + 1 WHERE id = $1"
        )
        .bind(rel_id)
        .execute(pool)
        .await;
    }
} 
