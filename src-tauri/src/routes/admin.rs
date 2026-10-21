use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::{
        audit::admin_audit,
        auth::{admin_only, auth_middleware, AppState},
    },
    models::merchant::MerchantPublic,
    utils::{
        card_gen::generate_api_key,
        db_guard, mq,
        plan::{extend_plan, ExtendOutcome},
    },
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get, patch},
    Extension, Json, Router,
};
use crate::utils::jwt::Claims;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct MerchantQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub keyword: Option<String>,
    pub plan: Option<String>,
}

pub fn admin_router_with_state(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/merchants", get(list_merchants).post(create_merchant))
        .route("/admin/merchants/:id", delete(delete_merchant))
        .route("/admin/merchants/:id/status", patch(update_merchant_status))
        .route("/admin/merchants/:id/plan", patch(update_merchant_plan))
        .route("/admin/merchants/:id/renew", patch(renew_merchant_plan))
        // 管理员自己的账号：改密码。
        //
        // 改造前**根本没有这个路由** —— 管理员忘记密码或怀疑泄露时，
        // 唯一的办法是直接连库改 `admins.password_hash`。
        // 商户端一直有这个接口（merchant.rs::change_password），管理员端没有，
        // 于是"权限最大的那类账号反而不能自助改密"。
        .route("/admin/password", patch(change_admin_password))
        // 审计日志查询（写入由 middleware::audit 自动完成，不需要单独接口）
        .route("/admin/audit-logs", get(list_audit_logs))
        .route("/admin/stats", get(get_stats))
        .route("/admin/stats/trends", get(get_trends))
        // 这里曾有两个「管理员 API Key」接口（/admin/api-key、/admin/api-key/regenerate），已删。
        //
        // 删的原因：它们是半成品，三层都缺 ——
        //   1. 依赖的 `admins.api_key` 列**从来没有被任何迁移创建过**，
        //      接口必然报「column "api_key" does not exist」；
        //   2. 全仓库没有任何中间件消费管理员的 api key（`/v1/*` 用的是**商户**的 key）；
        //   3. 前端页面 `/admin/api-docs` 没有菜单入口，用户根本到不了。
        // 它们文档化的 /v1/activate|verify|unbind 本来就是商户调的业务接口 ——
        // 管理员没有 app、没有卡密，本来就不需要 api key。
        // 见 .workbuddy/batch7-admin-apikey-cleanup.md
        // 审计挂最内层：在鉴权 / admin_only 之后跑，Claims 一定已注入，
        // 且能拿到 handler 返回后的状态码。所有管理端写操作自动留痕。
        .route_layer(middleware::from_fn_with_state(state.clone(), admin_audit))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn list_merchants(
    State(state): State<AppState>,
    Query(q): Query<MerchantQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;
    let keyword = q.keyword.unwrap_or_default();
    let keyword = &keyword[..keyword.len().min(100)]; // 限制搜索关键词长度
    let like = format!("%{}%", keyword);
    let plan_filter = q.plan.as_deref().unwrap_or("");

    // ⚠️ 这四段查询曾用 `.unwrap_or((0,))` / `.unwrap_or_default()`：
    // 管理端的商户列表静默变空。管理员打开后台看到「没有商户」，
    // 这是最容易引发误操作的一类假象 —— 有人真的会据此去"重新创建"账号。
    // 列表页没有合理的降级：给真数据，或者明确报错。
    let (total, merchants) = if plan_filter.is_empty() {
        let total: (i64,) = match sqlx::query_as(
            "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1",
        )
        .bind(&like)
        .fetch_one(&state.pool)
        .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("统计商户总数失败: err={}", e);
                return db_guard::server_busy();
            }
        };
        let rows: Vec<crate::models::merchant::Merchant> = match sqlx::query_as(
            "SELECT * FROM merchants WHERE username ILIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(&like)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询商户列表失败: err={}", e);
                return db_guard::server_busy();
            }
        };
        (total.0, rows)
    } else {
        let total: (i64,) = match sqlx::query_as(
            "SELECT COUNT(*) FROM merchants WHERE username ILIKE $1 AND plan = $2",
        )
        .bind(&like)
        .bind(plan_filter)
        .fetch_one(&state.pool)
        .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("统计商户总数（按套餐过滤）失败: plan={} err={}", plan_filter, e);
                return db_guard::server_busy();
            }
        };
        let rows: Vec<crate::models::merchant::Merchant> = match sqlx::query_as(
            "SELECT * FROM merchants WHERE username ILIKE $1 AND plan = $2 ORDER BY created_at DESC LIMIT $3 OFFSET $4",
        )
        .bind(&like)
        .bind(plan_filter)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询商户列表（按套餐过滤）失败: plan={} err={}", plan_filter, e);
                return db_guard::server_busy();
            }
        };
        (total.0, rows)
    };

    // 解密 email 和 api_key
    let public: Vec<MerchantPublic> = merchants.into_iter().map(|mut m| {
        if let Ok(plain) = EncryptedFieldsOps::decrypt_merchant_email(&state.encryptor, &m.email) {
            m.email = plain;
        } else {
            tracing::warn!("解密商户 {} email 失败", m.id);
        }
        if let Ok(plain) = EncryptedFieldsOps::decrypt_merchant_api_key(&state.encryptor, &m.api_key) {
            m.api_key = plain;
        } else {
            tracing::warn!("解密商户 {} api_key 失败", m.id);
        }
        m.into()
    }).collect();

    Json(json!({
        "success": true,
        "data": public,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

#[derive(Deserialize)]
pub struct CreateMerchantRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

async fn create_merchant(
    State(state): State<AppState>,
    Json(body): Json<CreateMerchantRequest>,
) -> Json<Value> {
    if body.username.trim().is_empty() {
        return Json(json!({"success": false, "message": "用户名不能为空"}));
    }
    if body.email.trim().is_empty() || !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }
    if body.password.len() < 6 {
        return Json(json!({"success": false, "message": "密码至少 6 位"}));
    }

    // 查重
    //
    // ⚠️ 两处都曾用 `.unwrap_or(None)`：查询失败 → 判定「不重复」→ 继续创建。
    // 和 `auth.rs` 的 register 同一类问题：绕过了唯一性检查，
    // 在最坏情况下产生重名商户（或重名邮箱）。
    let exists = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE username = $1")
            .bind(&body.username)
            .fetch_optional(&state.pool),
        "检查用户名是否已存在（admin 创建商户）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };
    if exists.is_some() {
        return Json(json!({"success": false, "message": "用户名已存在"}));
    }
    let exists_email = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE email_encrypted = $1")
            .bind(&body.email) // 邮件加密前可先模糊匹配
            .fetch_optional(&state.pool),
        "检查邮箱是否已被使用（admin 创建商户）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };
    if exists_email.is_some() {
        return Json(json!({"success": false, "message": "邮箱已被使用"}));
    }

    let password_hash = match bcrypt::hash(&body.password, bcrypt::DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    let merchant_id = Uuid::new_v4();

    // ── 加密 + 插商户 + 记加密日志：同一个事务 ─────────────────────────────
    //
    // 这段里有两个独立的问题，一起修：
    //
    // 1. **key_id 前缀本表写错**：这里原来是 `merchant_apikey_{id}`
    //    （`api` 和 `key` 之间少一个下划线），而其它所有入口
    //    （`EncryptedFieldsOps::encrypt_merchant_api_key_tx`、`routes/merchant.rs`）
    //    都用 `merchant_api_key_{id}`。密文自带 key_id，所以**解密不受影响**，
    //    但同一个字段在日志里出现两种 key_id 会让「按 key_id 检索/统计」失真 ——
    //    `merchant.rs:323` 的注释专门警告过这件事。改用统一前缀。
    //    （存量数据不受影响：`decrypt` 从密文第一段自取 key_id。）
    //
    // 2. **压根没写加密日志**：管理员建号这条路径建出来的商户，在
    //    `encrypted_fields_log` 里一个字都没有 —— 而那张表是「按行反查用了
    //    哪个密钥版本」的索引。注册（`auth.rs`）和第三方登录（`oauth.rs`）都有，
    //    只有这里漏了，属于「安静地少做一件事」。现在用 `_tx` 变体补上两条
    //    （email + api_key），并和商户 INSERT 同生共死。
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("创建商户时开启事务失败: err={}", e);
            return Json(json!({"success": false, "message": "创建失败，请稍后重试"}));
        }
    };

    // 加密并哈希敏感字段（key_id 由 helper 统一生成，别再手写前缀）
    let encrypted_email = match EncryptedFieldsOps::encrypt_merchant_email_tx(
        &mut *tx,
        &state.encryptor,
        merchant_id,
        &body.email,
    )
    .await
    {
        Ok(v) => v,
        Err(_) => return Json(json!({"success": false, "message": "加密邮箱失败"})),
    };
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);

    let raw_api_key = generate_api_key();
    let encrypted_api_key = match EncryptedFieldsOps::encrypt_merchant_api_key_tx(
        &mut *tx,
        &state.encryptor,
        merchant_id,
        &raw_api_key,
    )
    .await
    {
        Ok(v) => v,
        Err(_) => return Json(json!({"success": false, "message": "生成 API Key 失败"})),
    };
    let api_key_hash = EncryptedFieldsOps::generate_hash(&raw_api_key);

    let result = sqlx::query(
        "INSERT INTO merchants
           (id, username, email_encrypted, email_hash, password_hash,
            api_key_encrypted, api_key_hash, email_verified, created_by_admin)
         VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, TRUE)",
    )
    .bind(merchant_id)
    .bind(&body.username)
    .bind(&encrypted_email)
    .bind(&email_hash)
    .bind(&password_hash)
    .bind(&encrypted_api_key)
    .bind(&api_key_hash)
    .execute(&mut *tx)
    .await;

    match result {
        Ok(_) => {
            // 提交失败必须报错，不能像以前那样一律回 success ——
            // 「接口说创建成功、但账号其实没落库」是最难排查的一类问题
            if let Err(e) = tx.commit().await {
                tracing::error!("提交创建商户事务失败: username={} err={}", body.username, e);
                return Json(json!({"success": false, "message": "创建失败，请稍后重试"}));
            }
            Json(json!({
                "success": true,
                "message": "商户创建成功",
                "data": {
                    "id": merchant_id.to_string(),
                    "username": body.username,
                    "email": body.email,
                    "api_key": raw_api_key,
                }
            }))
        }
        // INSERT 失败 → `tx` 析构回滚，两条日志一起消失（不会留孤儿）
        Err(e) => db_guard::internal_error("创建商户", e),
    }
}

async fn delete_merchant(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    // 仅允许删除管理员创建的商户
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「商户不存在」→ 拒绝删除。
    // 授权判定这里方向是收严的（不会误删），但「商户不存在」是**假结论**：
    // 商户明明在列表里，管理员却被告知不存在，这是最让人困惑的一类提示。
    let target = match db_guard::optional(
        sqlx::query_as::<_, (bool,)>("SELECT created_by_admin FROM merchants WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool),
        "查询商户来源（删除前权限校验）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => r,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "商户不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if !target.0 {
        return Json(json!({
            "success": false,
            "message": "该商户由用户自助注册，不允许删除。如需禁用请使用禁用功能。"
        }));
    }

    // apps、cards、activations、messages 等关联表均有 ON DELETE CASCADE
    // 只需删除商户主记录即可，其余自动级联清理
    let result = sqlx::query("DELETE FROM merchants WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!("管理员删除了商户 {}", id);
            Json(json!({"success": true, "message": "商户已删除，相关应用和卡密数据已一并清理"}))
        }
        Ok(_) => Json(json!({"success": false, "message": "删除失败"})),
        Err(e) => db_guard::internal_error("删除商户", e),
    }
}

async fn update_merchant_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let status = match body.get("status").and_then(|s| s.as_str()) {
        Some(s) if s == "active" || s == "disabled" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效状态"})),
    };

    let result =
        sqlx::query("UPDATE merchants SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(&status)
            .bind(id)
            .execute(&state.pool)
            .await;

    match result {
        Ok(_) => {
            // ── 禁用时必须吊销已签发令牌 ────────────────────────────────────
            //
            // 不吊销的后果：封号封不住。JWT 无状态，被禁用的商户手里那张
            // access token 还能继续用最多 2 小时，而且 refresh 接口里
            // 「账号是否 active」的检查只能拦住 **refresh**，拦不住已经发出去的
            // access token —— 也就是说「封号」在最长 2 小时内完全不生效。
            //
            // ⚠️ **只在禁用时吊销，启用时不吊销**。
            // 启用（disabled → active）如果也推进版本号，会让刚被恢复的商户
            // 一登录就立刻掉线（他之前那张刚签的 token 版本号不匹配了），
            // 而这一步本没有任何安全目的。吊销是「收回权限」的动作，
            // 不是「状态变更」的附属品。
            if status == "disabled" {
                if let Err(e) = crate::utils::jwt::revoke_user_tokens(
                    &state.pool,
                    &mut state.redis.clone(),
                    "merchant",
                    &id,
                )
                .await
                {
                    // 状态**已经改成 disabled 了**，所以这里的失败不改变封禁效果
                    // （refresh 走不通了），只是「已发出的 access token 还能撑 2 小时」。
                    // 如实告知，让管理员知道需要留意。
                    tracing::error!("禁用商户后吊销令牌失败: merchant_id={} err={}", id, e);
                    return Json(json!({
                        "success": true,
                        "message": "状态已更新，但令牌吊销失败，该商户的旧会话可能在 2 小时内仍可用"
                    }));
                }
            }
            Json(json!({"success": true, "message": "状态已更新"}))
        }
        Err(e) => db_guard::internal_error("更新商户", e),
    }
}

async fn update_merchant_plan(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let plan = match body.get("plan").and_then(|s| s.as_str()) {
        Some(s) if s == "free" || s == "pro" => s.to_string(),
        _ => return Json(json!({"success": false, "message": "无效套餐，仅支持 free / pro"})),
    };

    // expires_days: 仅 pro 有效，None 表示永久，0 表示立即到期
    let expires_days = body.get("expires_days").and_then(|v| v.as_i64());

    // ── 语义统一（见 utils::plan 的模块注释）─────────────────────────────
    //
    // 这里曾经是 `plan_expires_at = NOW() + N days`，即**覆盖**：
    // 一个还剩 100 天的商户，管理员选 30 天会把它**缩短**成 30 天。
    // 界面上的按钮叫「升专业版」，看起来只是"开通"，没人会想到它在减天数。
    //
    // 现在与商户自助续费、管理端续期共用 `plan::extend_plan`：
    //   · 免费版 / 已过期 → 从现在起算 N 天
    //   · 专业版在有效期内 → 在剩余时间上叠加 N 天
    //   · 已是永久版 → 不修改（永久不该被天数覆盖）
    //
    // 「设为永久」「降为免费」仍是**赋值**语义，留在下面各自的分支里 ——
    // 那是管理员的显式意图，与"延长"不是一回事。
    let result = if plan == "pro" {
        match expires_days {
            Some(days) if days > 0 => {
                let mut conn = match state.pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => return db_guard::internal_error("更新商户套餐", e),
                };
                let outcome = extend_plan(&mut conn, id, days).await;
                drop(conn); // 先归还连接，后面的 MQ 发布不需要它

                match outcome {
                    Ok(ExtendOutcome::Extended { expires_at }) => {
                        // 升级 / 续期后恢复可能被降级禁用的 apps 与卡密。
                        // 失败只记日志：时长已经写进库了，通知晚到不影响结果。
                        if let Err(e) = mq::publish_upgrade(&state.mq_channel, &id.to_string()).await {
                            tracing::error!("发布升级恢复消息失败 {}: {}", id, e);
                        }
                        return Json(json!({
                            "success": true,
                            "message": format!("已延长 {} 天，到期时间 {}", days, expires_at.format("%Y-%m-%d %H:%M")),
                            "data": { "plan_expires_at": expires_at }
                        }));
                    }
                    // 永久版：不写库，如实告知（否则就是"点了成功、实际没变"）
                    Ok(ExtendOutcome::Permanent) => {
                        return Json(json!({
                            "success": false,
                            "message": "该商户已是永久专业版，无需设置有效期"
                        }))
                    }
                    Ok(ExtendOutcome::NotFound) => {
                        return Json(json!({"success": false, "message": "商户不存在"}))
                    }
                    Err(e) => return db_guard::internal_error("更新商户套餐", e),
                }
            }
            // 「设为永久」：显式赋值，SET plan_expires_at = NULL
            _ => {
                sqlx::query(
                    "UPDATE merchants
                     SET plan = $1,
                         plan_expires_at = NULL,
                         updated_at = NOW()
                     WHERE id = $2",
                )
                .bind(&plan)
                .bind(id)
                .execute(&state.pool)
                .await
            }
        }
    } else {
        // 手动降为免费版，清空到期时间
        sqlx::query(
            "UPDATE merchants
             SET plan = $1,
                 plan_expires_at = NULL,
                 updated_at = NOW()
             WHERE id = $2",
        )
        .bind(&plan)
        .bind(id)
        .execute(&state.pool)
        .await
    };

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let msg = if plan == "pro" {
                // 走到这里只剩「设为永久」一种情况（pro + 天数的分支上面已经 return）
                if let Err(e) = mq::publish_upgrade(&state.mq_channel, &id.to_string()).await {
                    tracing::error!("发布升级恢复消息失败 {}: {}", id, e);
                }
                "已设为永久专业版".to_string()
            } else {
                // ⚠️ 这一支曾经**什么都不发**，于是同一个「降为 free」出现两种行为：
                //   · 系统到期降级（worker）→ 超额 apps / 卡密会被禁用
                //   · 管理员手动降级（这里）→ 超额 apps / 卡密原样留着
                // 管理员以为已经降级到位，实际商户仍持有超额资源；而且因为 plan 已改，
                // 配额检查按 free 算（建不了新的），存量却不受限 —— 状态更难解释。
                // 补发**仅清理**消息（不是 publish_downgrade）：
                // 上面那条 UPDATE 已经把 plan 改成 free 了，worker 只需把超额 apps / 卡密禁掉。
                // 用 publish_downgrade 会被 worker 的预检挡掉 —— 那条路径要求「商户仍是 pro」，
                // 而且它拿 `updated_at > issued_at` 判消息过期，而这里刚把 updated_at 设成 NOW()，
                // issued_at 又是秒级截断的，条件恒成立、消息次次被跳过（实测踩到过）。
                if let Err(e) = mq::publish_cleanup(&state.mq_channel, &id.to_string()).await {
                    tracing::error!("发布降级清理消息失败 {}: {}", id, e);
                }
                "已降级为免费版".to_string()
            };
            Json(json!({"success": true, "message": msg}))
        }
        Ok(_) => Json(json!({"success": false, "message": "商户不存在"})),
        Err(e) => db_guard::internal_error("更新商户", e),
    }
}

/// 管理端「续期」：在商户当前到期时间上**叠加**天数。
///
/// 与上面 `update_merchant_plan` 的关系：现在两者都走 [`plan::extend_plan`]，
/// 语义完全一致（免费的从现在起算、有效期内叠加、已过期从现在起算）。
/// 之所以保留两个入口，是因为界面上的入口不同：
///   * 「升专业版」：给 free 商户开通，或直接给一个天数；
///   * 「续期」：只对"专业版且非永久"的行显示，专治"快到期 / 已到期"。
/// 两条路最终落到同一个函数，就不会再出现"同一个动作两种算法"的老问题。
///
/// 与商户自助续费（`payments.rs` 支付回调）也是同一个函数 —— 那条路径以前用
/// `COALESCE(plan_expires_at, NOW())`，已过期的商户付钱后到期时间仍在过去，
/// 下一轮扫描立刻降级，等于白付。
async fn renew_merchant_plan(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Json<Value> {
    // days 必填，且限定 1 ~ 3650（10 年）：挡住手滑多打一个 0 把商户续成百年会员
    let days = match body.get("days").and_then(|v| v.as_i64()) {
        Some(d) if d > 0 && d <= 3650 => d,
        Some(_) => {
            return Json(json!({"success": false, "message": "续期天数需在 1 ~ 3650 天之间"}))
        }
        None => return Json(json!({"success": false, "message": "请填写续期天数"})),
    };

    // 先查一次，只为**给出准确文案**：
    // 「商户不存在」「不是专业版」「已是永久」三种情况的下一步动作完全不同，
    // 只靠 UPDATE 的 rows_affected 只能回一句含糊的"失败"。
    //
    // 注意这不是"读-改-写"的竞态点：真正的判定与写入在 `extend_plan` 的一条
    // SQL 里完成（永久版会被 WHERE 挡住），这里只负责措辞。
    let target = match db_guard::optional(
        sqlx::query_as::<_, (String, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT plan, plan_expires_at FROM merchants WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&state.pool),
        "查询商户套餐（续期前校验）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => r,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "商户不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let (current_plan, expires_at) = target;
    if current_plan != "pro" {
        return Json(json!({
            "success": false,
            "message": "该商户不是专业版，请先用「升专业版」设置有效期"
        }));
    }
    // plan_expires_at IS NULL = 永久专业版。`extend_plan` 也会挡住它，
    // 但在这里先拦能给出更具体的提示（而不是让管理员等到返回后自己猜）。
    if expires_at.is_none() {
        return Json(json!({
            "success": false,
            "message": "该商户已是永久专业版，无需续期"
        }));
    }

    let mut conn = match state.pool.acquire().await {
        Ok(c) => c,
        Err(e) => return db_guard::internal_error("商户续期", e),
    };
    let outcome = extend_plan(&mut conn, id, days).await;
    drop(conn);

    match outcome {
        Ok(ExtendOutcome::Extended { expires_at: new_expires_at }) => {
            tracing::info!(
                "管理员为商户 {} 续期 {} 天，新到期时间 {}",
                id,
                days,
                new_expires_at
            );
            // 与商户自助续费保持一致：投递恢复消息，把可能因降级被禁用的 apps / 卡密放回来。
            // 这条消息不是必需的（真正到期降级的商户 plan 已被改成 free，走不到这里），
            // 只兜「清理做完、plan 还没改就被中断」这种半途状态，失败不影响续期结果本身。
            if let Err(e) = mq::publish_upgrade(&state.mq_channel, &id.to_string()).await {
                tracing::error!("发布续期恢复消息失败 {}: {}", id, e);
            }
            Json(json!({
                "success": true,
                "message": format!("已续期 {} 天", days),
                "data": { "plan_expires_at": new_expires_at }
            }))
        }
        // 预检之后被别的管理员改成永久版 / 被删除：如实说"状态变了"，不要硬算
        Ok(ExtendOutcome::Permanent) => Json(json!({
            "success": false,
            "message": "该商户刚被改为永久专业版，无需续期"
        })),
        Ok(ExtendOutcome::NotFound) => Json(json!({
            "success": false,
            "message": "商户套餐已变更（可能已被删除），请刷新后重试"
        })),
        Err(e) => db_guard::internal_error("商户续期", e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 管理员账号：改密码
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AdminChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// 管理员修改自己的密码。
///
/// 与商户端 `merchant.rs::change_password` 的逻辑保持一致（校验原密码 →
/// 写新哈希 → 吊销全部令牌 → 给当前设备补发一对新令牌），差别只在表名与角色。
///
/// 为什么必须先吊销：JWT 是无状态的，签名对即有效。不吊销的话**改密码没有任何
/// 安全效果** —— 已经泄露的 token 照样能用满 2 小时，refresh token 还能滚动续期
/// 7 天以上。管理员账号能改套餐、改支付密钥，这条尤其不能省。
async fn change_admin_password(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<AdminChangePasswordRequest>,
) -> Json<Value> {
    if body.new_password.chars().count() < 8 {
        return Json(json!({"success": false, "message": "新密码至少 8 位"}));
    }
    // bcrypt 只使用前 72 字节，超长密码会被静默截断（"看起来设了 100 位，
    // 实际只有前 72 字节有效"）。这里直接拒绝，避免制造这种错觉。
    if body.new_password.len() > 72 {
        return Json(json!({"success": false, "message": "新密码过长（最多 72 字节）"}));
    }
    if body.new_password == body.old_password {
        return Json(json!({"success": false, "message": "新密码不能与原密码相同"}));
    }

    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效的用户标识"})),
    };

    let (password_hash, email) = match db_guard::optional(
        sqlx::query_as::<_, (String, String)>(
            "SELECT password_hash, email FROM admins WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&state.pool),
        "查询管理员账号",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => r,
        db_guard::QueryOutcome::NotFound => {
            // 令牌里的 sub 指向一个不存在的管理员：账号已被删除，但 token 还没过期。
            return Json(json!({"success": false, "message": "账号不存在，请重新登录"}));
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    // ⚠️ bcrypt 是**同步 CPU 密集**调用（cost 12 约 250ms）。直接 .await 之前跑在
    // tokio worker 线程上，几个并发就能把线程池占满、拖慢全站所有接口。
    // 这里用 spawn_blocking，把 CPU 活挪到阻塞线程池。
    // （auth.rs 的登录路径还是同步调用 —— 那属于体检报告里的 P1，另行统一。）
    let old_input = body.old_password.clone();
    let hash_in_db = password_hash.clone();
    let valid = tokio::task::spawn_blocking(move || {
        bcrypt::verify(&old_input, &hash_in_db).unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    if !valid {
        return Json(json!({"success": false, "message": "原密码错误"}));
    }

    let new_input = body.new_password.clone();
    let new_hash = match tokio::task::spawn_blocking(move || {
        bcrypt::hash(new_input, bcrypt::DEFAULT_COST)
    })
    .await
    {
        Ok(Ok(h)) => h,
        _ => return Json(json!({"success": false, "message": "密码加密失败，请稍后重试"})),
    };

    match sqlx::query("UPDATE admins SET password_hash = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_hash)
        .bind(id)
        .execute(&state.pool)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {}
        Ok(_) => return Json(json!({"success": false, "message": "账号不存在，请重新登录"})),
        Err(e) => return db_guard::internal_error("更新管理员密码", e),
    }

    // 吊销该管理员的所有已签发令牌（token_version + 1）。
    // 失败要如实告知：密码确实改了，但"把别人踢下线"这件事没做成 ——
    // 这两件事必须分开说，否则用户以为旧会话已经失效。
    let new_ver = match crate::utils::jwt::revoke_user_tokens(
        &state.pool,
        &mut state.redis.clone(),
        "admin",
        &id,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("管理员改密码后吊销令牌失败: admin_id={} err={}", id, e);
            return Json(json!({
                "success": false,
                "message": "密码已修改，但会话清理失败，请重新登录一次以确认安全"
            }));
        }
    };

    // 给当前这台设备补发一对新令牌（旧版号已作废，所以它是有效的）
    let token = crate::utils::jwt::generate_token(&id, "admin", &email, new_ver, &state.jwt_secret);
    let refresh = crate::utils::jwt::generate_refresh_token(
        &id,
        "admin",
        &email,
        new_ver,
        &state.jwt_secret,
    );

    match (token, refresh) {
        (Ok(token), Ok(refresh_token)) => Json(json!({
            "success": true,
            "message": "密码已修改，其他设备需重新登录",
            "token": token,
            "refresh_token": refresh_token
        })),
        // 签发失败不影响"密码已改"这个事实，只影响当前设备是否需要重登
        _ => Json(json!({"success": true, "message": "密码已修改，请重新登录"})),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 审计日志查询
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuditLogQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    /// 只看某个管理员的操作
    pub admin_id: Option<Uuid>,
    /// 按路由模板 / 实际路径模糊匹配（如 "merchants" 或 "/admin/plan-configs"）
    pub keyword: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AuditLogRow {
    id: i64,
    admin_id: Option<Uuid>,
    admin_email: Option<String>,
    method: String,
    route: String,
    path: String,
    status: i16,
    detail: Option<Value>,
    ip: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    total_count: i64,
}

/// 查询审计日志。
///
/// 写入由 `middleware::audit` 自动完成，这里只负责读。没有任何写入接口 ——
/// 审计日志**只能追加**，管理端不提供修改/删除，这是审计可信的前提
/// （真要归档由 DBA 按时间分区处理）。
async fn list_audit_logs(
    State(state): State<AppState>,
    Query(q): Query<AuditLogQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;

    // 关键词限长：避免超长串拼进 LIKE 拖慢扫描
    let keyword: String = q.keyword.unwrap_or_default().trim().chars().take(100).collect();
    let like = format!("%{}%", keyword);

    let rows: Vec<AuditLogRow> = match sqlx::query_as(
        "SELECT id, admin_id, admin_email, method, route, path, status, detail, ip,
                created_at, COUNT(*) OVER() AS total_count
           FROM admin_audit_logs
          WHERE ($1::uuid IS NULL OR admin_id = $1)
            AND ($2 = '' OR route ILIKE $3 OR path ILIKE $3)
          ORDER BY created_at DESC
          LIMIT $4 OFFSET $5",
    )
    .bind(q.admin_id)
    .bind(&keyword)
    .bind(&like)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => return db_guard::internal_error("查询审计日志", e),
    };

    let total = rows.first().map(|r| r.total_count).unwrap_or(0);
    let data: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "admin_id": r.admin_id,
                "admin_email": r.admin_email,
                "method": r.method,
                "route": r.route,
                "path": r.path,
                "status": r.status,
                "detail": r.detail,
                "ip": r.ip,
                "created_at": r.created_at,
            })
        })
        .collect();

    Json(json!({
        "success": true,
        "data": data,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

async fn get_stats(State(state): State<AppState>) -> Json<Value> {
    // ⚠️ 这五段曾各自 `.unwrap_or((0,))`。大盘数字静默变 0 有两个层面的坏处：
    //   1. 运维会以为「平台今天没有新增」，据此做判断（比如去检查推广渠道）；
    //   2. 五个数字**互相自洽**（都是 0），界面上看不出任何异常 ——
    //      这和一个「刚上线、确实没数据」的库长得一模一样。
    //
    // 这里不逐项降级：任何一个查不出来，整个大盘就是不可信的，报错更诚实。
    macro_rules! stat_count {
        ($sql:expr, $what:expr) => {
            match db_guard::scalar(
                sqlx::query_as::<_, (i64,)>($sql).fetch_one(&state.pool),
                $what,
            )
            .await
            {
                db_guard::ScalarOutcome::Found(v) => v.0,
                db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
            }
        };
    }

    let merchant_count = stat_count!("SELECT COUNT(*) FROM merchants", "统计商户总数（大盘）");
    let card_count = stat_count!("SELECT COUNT(*) FROM cards", "统计卡密总数（大盘）");
    let activation_count = stat_count!("SELECT COUNT(*) FROM activations", "统计激活总数（大盘）");
    let active_card_count = stat_count!("SELECT COUNT(DISTINCT card_id) FROM activations", "统计已激活卡密数（大盘）");
    let app_count = stat_count!("SELECT COUNT(*) FROM apps", "统计应用总数（大盘）");

    Json(json!({
        "success": true,
        "data": {
            "merchants": merchant_count,
            "total_cards": card_count,
            "active_cards": active_card_count,
            "total_activations": activation_count,
            "total_apps": app_count
        }
    }))
}

/// 每日增量趋势（近 30 天）
async fn get_trends(State(state): State<AppState>) -> Json<Value> {
    // ⚠️ 三段曾各自 `.unwrap_or_default()`：趋势图静默变成一条平线。
    // 趋势图是最容易被误读的图表 —— 一条平线既可以是「这几天真的没新增」，
    // 也可以是「数据库连不上」。两者在界面上**完全无法区分**，必须让它报错。
    let merchants: Vec<(chrono::NaiveDate, i64)> = match sqlx::query_as(
        r#"SELECT DATE(created_at) AS day, COUNT(*)::bigint AS cnt
           FROM merchants WHERE created_at >= NOW() - INTERVAL '30 days'
           GROUP BY day ORDER BY day"#,
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询商户趋势失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let apps: Vec<(chrono::NaiveDate, i64)> = match sqlx::query_as(
        r#"SELECT DATE(created_at) AS day, COUNT(*)::bigint AS cnt
           FROM apps WHERE created_at >= NOW() - INTERVAL '30 days'
           GROUP BY day ORDER BY day"#,
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询应用趋势失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let cards: Vec<(chrono::NaiveDate, i64)> = match sqlx::query_as(
        r#"SELECT DATE(created_at) AS day, COUNT(*)::bigint AS cnt
           FROM cards WHERE created_at >= NOW() - INTERVAL '30 days'
           GROUP BY day ORDER BY day"#,
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询卡密趋势失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    // 补全 30 天空白天
    let fill_days = |rows: Vec<(chrono::NaiveDate, i64)>| -> Vec<Value> {
        let mut map: std::collections::HashMap<chrono::NaiveDate, i64> = rows.into_iter().collect();
        let mut filled = Vec::new();
        for i in (0..30).rev() {
            let d = (chrono::Utc::now() - chrono::Duration::days(i)).date_naive();
            let cnt = map.remove(&d).unwrap_or(0);
            filled.push(json!({"date": d.to_string(), "count": cnt}));
        }
        filled
    };

    Json(json!({
        "success": true,
        "data": {
            "merchants": fill_days(merchants),
            "apps": fill_days(apps),
            "cards": fill_days(cards),
        }
    }))
}

// 这里曾有两个 handler（get_admin_api_key / regenerate_admin_api_key），已随
// `/admin/api-key` 路由一起删除。删除理由见上方路由注册处的说明。
//
// 保留一条经验：原先 get_admin_api_key 里叠了 `.unwrap_or(None)` +
// `.unwrap_or_default()` 两层静默 —— 查询失败会被压成**空字符串**，在界面上与
// 「还没生成过 Key」长得一模一样。这类「空串当有效值」的默认值在任何需要区分
// 「查不到」与「查不了」的地方都要避免；它们只是把错误藏起来，不是处理错误。
