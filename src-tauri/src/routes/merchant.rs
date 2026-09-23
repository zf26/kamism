use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{AppState, auth_middleware},
    utils::{
        db_guard,
        jwt::{generate_refresh_token, generate_token, Claims},
    },
};
use axum::{
    extract::{Query, State},
    middleware,
    routing::{get, post},
    Extension, Json, Router,
};
use bcrypt::{hash, DEFAULT_COST};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Deserialize)]
pub struct DashboardQuery {
    pub range: Option<String>,
}

pub fn merchant_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/merchant/profile", get(get_profile))
        .route("/merchant/dashboard-stats", get(dashboard_stats))
        .route("/merchant/change-password", post(change_password))
        .route("/merchant/regenerate-apikey", post(regenerate_api_key))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn get_profile(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 返回「用户不存在」。
    // 用户明明正登录着，却被告知自己不存在 —— 这是会让用户直接来报故障的提示，
    // 而排查时会先去查账号，查不出任何问题（因为账号是好的）。
    let merchant = match db_guard::optional(
        sqlx::query_as::<_, crate::models::merchant::Merchant>("SELECT * FROM merchants WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool),
        "查询当前商户",
    )
    .await
    {
        db_guard::QueryOutcome::Found(m) => m,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "用户不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    // 到这里 `merchant` 已经是 `Merchant`（不是 Option）—— 三种情况都在上面分派完了。
    let public: crate::models::merchant::MerchantPublic = merchant.into();
    Json(json!({"success": true, "data": public}))
}

async fn dashboard_stats(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<DashboardQuery>,
) -> Json<Value> {
    let merchant_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    // 根据 range 参数决定时间区间和分组粒度
    let (interval, trunc, label) = match q.range.as_deref().unwrap_or("week") {
        "month" => ("3 months", "week", "month"),
        "year"  => ("1 year",   "month", "year"),
        _       => ("7 days",   "day",   "week"),  // 默认周
    };
    let _ = label;

    // 1. 卡密使用率
    // ⚠️ 曾用 .unwrap_or_default()：查询失败时仪表盘的卡密状态环形图变空，
    // 商户看到「暂无卡密数据」，会以为卡密从未生成过（实际是统计查询挂了）。
    let card_stats: Vec<(String, i64)> = match sqlx::query_as(
        "SELECT status, COUNT(*) FROM cards WHERE merchant_id = $1 GROUP BY status",
    )
    .bind(merchant_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询仪表盘卡密状态统计失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    // 2. 激活趋势（动态粒度）
    let sql = format!(
        "SELECT DATE_TRUNC('{trunc}', activated_at)::date AS day, COUNT(*) AS cnt
         FROM activations
         WHERE card_id IN (SELECT id FROM cards WHERE merchant_id = $1)
           AND activated_at >= NOW() - INTERVAL '{interval}'
         GROUP BY day
         ORDER BY day",
        trunc = trunc,
        interval = interval,
    );
    // ⚠️ 曾用 .unwrap_or_default()：查询失败时趋势图静默变成一条空曲线，
    // 商户看到「近期激活量断崖式下跌到 0」，会误判业务出问题并去排查卡密，
    // 而实际只是趋势查询失败。
    let activation_trend: Vec<(chrono::NaiveDate, i64)> =
        match sqlx::query_as(&sql)
            .bind(merchant_id)
            .fetch_all(&state.pool)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询仪表盘激活趋势失败: err={}", e);
                return db_guard::server_busy();
            }
        };

    // 3. 设备分布
    // ⚠️ 曾用 .unwrap_or_default()：查询失败时设备分布表变空，
    // 商户看到「暂无可统计的设备」，会以为所有设备都没在用卡密。
    let device_dist: Vec<(String, i64)> = match sqlx::query_as(
        "SELECT a.app_name, COUNT(act.id) AS device_cnt
         FROM apps a
         LEFT JOIN cards c ON c.app_id = a.id AND c.merchant_id = $1
         LEFT JOIN activations act ON act.card_id = c.id
         WHERE a.merchant_id = $1
         GROUP BY a.app_name
         ORDER BY device_cnt DESC
         LIMIT 10",
    )
    .bind(merchant_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询仪表盘设备分布失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    Json(json!({
        "success": true,
        "data": {
            "card_stats": card_stats.iter().map(|(s, c)| json!({"status": s, "count": c})).collect::<Vec<_>>(),
            "activation_trend": activation_trend.iter().map(|(d, c)| json!({"date": d.to_string(), "count": c})).collect::<Vec<_>>(),
            "device_dist": device_dist.iter().map(|(app, c)| json!({"app": app, "count": c})).collect::<Vec<_>>(),
        }
    }))
}

async fn change_password(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<ChangePasswordRequest>,
) -> Json<Value> {
    if body.new_password.len() < 8 {
        return Json(json!({"success": false, "message": "新密码至少8位"}));
    }
    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 返回「用户不存在」。
    // 用户明明正登录着，却被告知自己不存在 —— 这是会让用户直接来报故障的提示，
    // 而排查时会先去查账号，查不出任何问题（因为账号是好的）。
    let merchant = match db_guard::optional(
        sqlx::query_as::<_, crate::models::merchant::Merchant>("SELECT * FROM merchants WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool),
        "查询当前商户",
    )
    .await
    {
        db_guard::QueryOutcome::Found(m) => m,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "用户不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let valid = bcrypt::verify(&body.old_password, &merchant.password_hash).unwrap_or(false);
    if !valid {
        return Json(json!({"success": false, "message": "原密码错误"}));
    }

    let new_hash = match hash(&body.new_password, DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    match sqlx::query(
        "UPDATE merchants SET password_hash = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&new_hash)
    .bind(id)
    .execute(&state.pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // ── 吊销全部已签发令牌 ──────────────────────────────────────────
            //
            // 为什么改密码必须吊销：JWT 是无状态的，签名对即有效。
            // 不吊销的话，**改密码不产生任何安全效果** —— 攻击者偷到的 token
            // 照样能用满 2 小时，refresh token 还能续 7 天并且滚动续期。
            // 用户改密码的心理预期是「把坏人踢出去」，不吊销等于骗他。
            //
            // 注意这里**不考虑保留当前会话**：改完密码前端会要求重新登录。
            // 理由是这个接口没有「当前设备标识」，无法区分「发起修改的这台」和
            // 「其他设备」—— 与其按 IP/UA 猜（会猜错，而且猜错的方向是
            // 「该踢的没踢」或「不该踢的踢了」），不如统一要求重登：
            // 行为可预测，代价只是多点一次登录。
            //
            // ⚠️ 吊销失败要如实告知，不能吞掉：否则用户以为坏人都被踢了，
            // 实际上旧 token 还活着 —— 这是典型的「安静地做错事」。
            //
            // 吊销粒度只有「该用户全部令牌」这一档（token_version 是个计数器），
            // 所以**当前这台设备也会被踢**。为了不让操作者自己莫名掉线，
            // 这里在吊销之后**立刻用新版号补签一对令牌返回**：
            //   - 当前设备：旧 token 作废，但响应里带了新的 → 前端替换即可，无感
            //   - 其他设备：手里的旧 token 版本号落后 → 被拒，必须重新登录
            // 这正是 token_version 方案的适用方式：区分不了设备，
            // 就用「先全踢、再给操作者补一张」达到等效效果。
            let new_ver = match crate::utils::jwt::revoke_user_tokens(
                &state.pool,
                &mut state.redis.clone(),
                "merchant",
                &id,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    // 注意密码**已经改成功了**，所以文案必须把两件事分开说，
                    // 不能简单讲「修改失败」让用户以为密码没变。
                    tracing::error!("改密码后吊销令牌失败: merchant_id={} err={}", id, e);
                    return Json(json!({
                        "success": false,
                        "message": "密码已修改，但会话清理失败，请重新登录一次以确认安全"
                    }));
                }
            };

            // 给操作者补发一对新令牌（用吊销后的版本号，因此它是有效的）
            let token = match generate_token(&merchant.id, "merchant", &merchant.email, new_ver, &state.jwt_secret) {
                Ok(t) => t,
                Err(_) => return Json(json!({
                    "success": true, "message": "密码已修改，请重新登录"
                })),
            };
            let refresh_token = match generate_refresh_token(
                &merchant.id, "merchant", &merchant.email, new_ver, &state.jwt_secret,
            ) {
                Ok(t) => t,
                Err(_) => return Json(json!({
                    "success": true, "message": "密码已修改，请重新登录"
                })),
            };

            Json(json!({
                "success": true,
                "message": "密码已修改，其他设备需重新登录",
                "token": token,
                "refresh_token": refresh_token
            }))
        }
        Ok(_) => Json(json!({"success": false, "message": "修改失败"})),
        Err(e) => {
            tracing::error!("更新密码失败: {}", e);
            Json(json!({"success": false, "message": "服务器错误，请稍后重试"}))
        }
    }
}

async fn regenerate_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    let new_key = crate::utils::card_gen::generate_api_key();

    // ── 必须同时写加密列和哈希列 ──────────────────────────────────────────
    //
    // ⚠️ 这里曾写 `UPDATE merchants SET api_key = $1` —— **merchants 表没有
    // `api_key` 这个列**（只有 `api_key_encrypted` + `api_key_hash`），
    // 于是这个接口 100% 失败：
    //   接口返回 {"message":"服务器错误，请稍后重试"}
    //   日志     ERROR ...: 关系 "merchants" 的 "api_key" 字段不存在
    //
    // 危害不只是「功能坏了」：外层文案是「服务器繁忙」这种**像临时故障**的
    // 说法，会让用户以为重试就好，反复点。而它实际上是确定性失败 ——
    // 用户唯一的自救手段（怀疑 Key 泄漏时换一个）彻底不可用。
    //
    // 为什么不能简单加一个 `api_key` 裸列了事（也试过这个方向）：
    //   全项目的 Key 校验走的是 `WHERE api_key_hash = $1`
    //   （`public_api.rs:81/729/922`、`db_guard.rs:12` 注释里都写着），
    //   只填裸列不会让新 Key 生效（校验查的是 hash 列）；
    //   反过来只填 hash 列，`get_profile`/admin 列表解密时会拿到空串。
    //   两列必须同生共死 —— 这正是 `auth.rs:249-290` 注册、`admin.rs:210-240`
    //   建号、`oauth.rs:389-430` 三方登录的既有写法，这里照抄它。
    //
    // key_id 用 `merchant_api_key_{id}`（与 `encrypt_merchant_api_key` 一致），
    // **不带版本后缀**：`decrypt` 从密文第一段自取 key_id（`kms.rs:229-239`），
    // 而这里没有轮密钥，后缀只会让同一个字段出现两种 key_id 造成混乱。
    let key_id = format!("merchant_api_key_{}", id);
    let encrypted_api_key = match state.encryptor.encrypt(&new_key, &key_id) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("重新生成 API Key 时加密失败: merchant_id={} err={}", id, e);
            return Json(json!({"success": false, "message": "服务器错误，请稍后重试"}));
        }
    };
    let api_key_hash = EncryptedFieldsOps::generate_hash(&new_key);

    // ── 用事务把「改 Key」+「记加密日志」绑在一起 ────────────────────────
    //
    // 加密日志若写在事务外（用 pool 自开短事务），一旦下面的事务回滚，
    // `encrypted_fields_log` 里就会留下一条指向「其实没换过 Key」的记录 ——
    // 就是 `encrypted_fields.rs:43-49` 警告的那类孤儿日志（那张表没有外键，
    // 数据库不会拦）。所以这里用 `log_encryption_tx` 复用同一个事务。
    let mut tx = match state.pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("重新生成 API Key 时开启事务失败: merchant_id={} err={}", id, e);
            return Json(json!({"success": false, "message": "服务器错误，请稍后重试"}));
        }
    };

    let updated = match sqlx::query(
        "UPDATE merchants SET api_key_encrypted = $1, api_key_hash = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(&encrypted_api_key)
    .bind(&api_key_hash)
    .bind(id)
    .execute(&mut *tx)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("重生成 API Key 失败: merchant_id={} err={}", id, e);
            let _ = tx.rollback().await;
            return Json(json!({"success": false, "message": "服务器错误，请稍后重试"}));
        }
    };

    if updated.rows_affected() == 0 {
        let _ = tx.rollback().await;
        return Json(json!({"success": false, "message": "重生成失败"}));
    }

    // 记录加密日志（复用事务）。失败则整体回滚：
    // 日志是「这个字段用哪个密钥版本加密的」的唯一凭据，缺了它以后可能解不开，
    // 不能容忍「Key 换了但日志没写」这种半成品状态。
    if let Err(e) =
        EncryptedFieldsOps::log_encryption_tx(&mut *tx, "merchants", id, "api_key", &key_id).await
    {
        tracing::error!("记录 API Key 加密日志失败，已回滚: merchant_id={} err={}", id, e);
        let _ = tx.rollback().await;
        return Json(json!({"success": false, "message": "服务器错误，请稍后重试"}));
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("提交 API Key 重生成事务失败: merchant_id={} err={}", id, e);
        return Json(json!({"success": false, "message": "服务器错误，请稍后重试"}));
    }

    // ── 重置 API Key 也要吊销会话 ────────────────────────────────────
    //
    // 为什么不吊销不行：api_key 是**暴露面积最大**的凭证 ——
    // 它会被写进客户端的请求（`X-Api-Key` 之类）、出现在各种第三方集成配置里、
    // 甚至贴进工单和聊天记录。用户点「重新生成」的动作本身就说明
    // 「我认为这个 key 已经泄漏了」。如果旧 key 泄漏了，那么**同一个
    // 泄漏渠道很可能也带走了 token**（比如整份配置文件被传出去），
    // 所以这里连同令牌一起作废。
    //
    // 权衡：这会让该商户的所有登录会话失效（含发起操作的这一台）。
    // 这里不补发新令牌 —— 与 change_password 不同，因为 api_key 的典型使用方是
    // **程序/脚本**而不是浏览器会话，用户点完之后本来就要去更新集成配置，
    // 顺手重登一次不算额外负担；而补发令牌会让「谁该更新配置」这件事变模糊。
    //
    // 顺序说明：放在事务**提交之后**。吊销是 Redis + DB 的另一组写入，
    // 不该拖长上面那个事务的持锁时间；而且「Key 已换、吊销失败」是可接受的
    // 半成功状态（下面会留 error 日志），反过来「吊销成功但 Key 没换成」
    // 才是要避免的。
    if let Err(e) = crate::utils::jwt::revoke_user_tokens(
        &state.pool,
        &mut state.redis.clone(),
        "merchant",
        &id,
    )
    .await
    {
        // 不返回失败：api_key **已经换成功了**，那才是用户要的结果。
        // 但必须留痕 —— 否则「重置了 key，会话却没清掉」这件事无人知晓。
        tracing::error!("重置 API Key 后吊销令牌失败: merchant_id={} err={}", id, e);
    }

    Json(json!({"success": true, "data": {"api_key": new_key}}))
}

