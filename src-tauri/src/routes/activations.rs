use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{AppState, auth_middleware},
    utils::{db_guard, jwt::Claims},
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{delete, get},
    Extension, Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ActivationWithCode {
    pub id: Uuid,
    pub card_id: Uuid,
    pub card_code: String,
    pub app_id: Uuid,
    pub device_id: String,
    pub device_name: Option<String>,
    pub ip_address: Option<String>,
    pub activated_at: DateTime<Utc>,
    pub last_verified_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct ActivationQuery {
    pub card_code: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub fn activations_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/activations", get(list_activations))
        .route("/activations/:id", delete(unbind_device))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

fn merchant_id_from_claims(claims: &Claims) -> Result<Uuid, Json<Value>> {
    Uuid::parse_str(&claims.sub)
        .map_err(|_| Json(json!({"success": false, "message": "无效用户ID"})))
}

async fn list_activations(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<ActivationQuery>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;
    let card_code_filter = q.card_code.as_deref().unwrap_or("").trim().to_lowercase();

    // ⚠️ 曾用 .unwrap_or_default()：激活记录查询失败时列表变空，
    // 商户在「激活记录」页看到「暂无数据」，会以为卡密从未被使用过，
    // 于是把卡密当成异常（未激活）处理 —— 而真正的问题是数据库查不到。
    let raw_activations: Vec<(Uuid, Uuid, String, Uuid, String, Option<String>, Option<String>, DateTime<Utc>, DateTime<Utc>)> = match sqlx::query_as(
        r#"SELECT a.id, a.card_id, c.code_encrypted, a.app_id, a.device_id_encrypted,
                  a.device_name, a.ip_address, a.activated_at, a.last_verified_at
           FROM activations a
           JOIN cards c ON c.id = a.card_id
           WHERE c.merchant_id = $1
           ORDER BY a.activated_at DESC
           LIMIT $2 OFFSET $3"#,
    )
    .bind(merchant_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询激活记录列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let mut activations = Vec::new();
    for (id, card_id, encrypted_code, app_id, encrypted_device_id, device_name, ip_address, activated_at, last_verified_at) in raw_activations {
        let card_code = EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &encrypted_code)
            .unwrap_or_else(|_| "[解密失败]".to_string());
        let device_id = EncryptedFieldsOps::decrypt_device_id(&state.encryptor, &encrypted_device_id)
            .unwrap_or_else(|_| "[解密失败]".to_string());

        if !card_code_filter.is_empty() && !card_code.to_lowercase().contains(&card_code_filter) {
            continue;
        }

        activations.push(ActivationWithCode {
            id, card_id, card_code, app_id, device_id,
            device_name, ip_address, activated_at, last_verified_at,
        });
    }

    // ⚠️ 曾用 .unwrap_or((0,))：数据库故障被压成 total=0，
    // 商户后台会看到「一条激活记录都没有」（列表明明有数据，总数却是 0），
    // 于是去怀疑筛选条件或数据丢失，而真相是库挂了 —— 日志里一个字都没有。
    let total: (i64,) = match db_guard::scalar(
        sqlx::query_as(
            "SELECT COUNT(*) FROM activations a JOIN cards c ON c.id = a.card_id WHERE c.merchant_id = $1",
        )
        .bind(merchant_id)
        .fetch_one(&state.pool),
        "统计商户激活记录总数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(v) => v,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    Json(json!({
        "success": true,
        "data": activations,
        "total": total.0,
        "page": page,
        "page_size": page_size
    }))
}

#[derive(sqlx::FromRow)]
struct ActivationCardId { card_id: Uuid }

async fn unbind_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    // ⚠️ 曾用 .unwrap_or(None)：数据库故障被压成「记录不存在或无权限」，
    // 商户点解绑会被告知这条记录不属于自己（其实是库连不上），
    // 会去找客服查权限，而真正的故障没有任何日志。
    let activation: Option<ActivationCardId> = match db_guard::optional(
        sqlx::query_as(
            r#"SELECT a.card_id FROM activations a
           JOIN cards c ON c.id = a.card_id
           WHERE a.id = $1 AND c.merchant_id = $2"#,
        )
        .bind(id)
        .bind(merchant_id)
        .fetch_optional(&state.pool),
        "查询待解绑的激活记录",
    )
    .await
    {
        db_guard::QueryOutcome::Found(a) => Some(a),
        // 查通了确实没有这行 —— 正常业务结论，保留原文案
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    let activation = match activation {
        Some(a) => a,
        None => return Json(json!({"success": false, "message": "记录不存在或无权限"})),
    };

    // 这里不必主动清 Redis 的 verify 缓存：
    // /v1/verify 命中缓存时会实时复核「这条激活记录是否还在」
    // （见 public_api.rs 的 judge_cached_entry），所以解绑在下次 verify 立即生效。
    // 商户后台拿不到 api_key / 卡密明文，本来也拼不出那个缓存 key。
    //
    // ── 为什么这三步必须在同一个事务里 ──────────────────────────────────────
    // 「删激活记录 → 数剩余设备 → 必要时把卡密恢复成 unused」是一个整体：
    // 前面任何一步没做，后面那步的结论就是错的（会按错误的剩余数去改卡密状态）。
    //
    // ⚠️ 这里曾经是「池上各自执行 + `let _ =` 丢掉错误」，DELETE 失败也照样回
    // 「设备已解绑」。也不能指望「commit 会拦住错误」来兜底 —— 这是实测过的坑：
    // 事务内某条语句失败后事务进入 aborted 状态，此时 sqlx 发出 COMMIT，
    // **PG 返回的命令标签是 `ROLLBACK` 而不是错误**，于是 `tx.commit()` 返回 Ok，
    // 整笔事务静默回滚，接口却报成功。
    // （探针与原始输出见 .workbuddy/batch8-swallowed-write-errors.md）
    let mut tx = match state.pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("解绑设备：开启事务失败: activation_id={} err={}", id, e);
            return db_guard::server_busy();
        }
    };

    let deleted = sqlx::query("DELETE FROM activations WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await;

    match deleted {
        Ok(r) if r.rows_affected() > 0 => {}
        Ok(_) => {
            // 前面刚带着商户归属条件查到过这行，这里为 0 只可能是并发（别的请求先删了）。
            // 结论同样是「这台设备已经不绑定了」，所以不报错；但记一笔便于排查。
            tracing::warn!(
                "解绑设备：待删的激活记录已不存在（疑似并发解绑）: activation_id={}",
                id
            );
        }
        Err(e) => {
            // tx 在函数返回时析构 → 自动回滚。这里必须回失败：DELETE 没做成，
            // 说「设备已解绑」就是撒谎（用户会以为新设备能绑上，实际绑不上）。
            tracing::error!("解绑设备：删除激活记录失败: activation_id={} err={}", id, e);
            return db_guard::server_busy();
        }
    }

    // 计数必须用事务内的连接，才能看到与上面 DELETE 同一份数据。
    // ⚠️ 曾用 .unwrap_or((0,))：计数失败被当成 0，于是把卡密状态恢复成 unused ——
    // 如果这张卡其实还被别的设备绑着，恢复 unused 会让「重复激活检测」失效，
    // 同一张卡能再次被绑定（**放松方向**的静默故障，没有任何人会发现）。
    //
    // 注意：上一版这里选择「跳过状态恢复而不是返回失败」，理由是「DELETE 已经成功，
    // 此时回失败是撒谎」。那个理由在当时是对的，但它成立的前提是 DELETE 真的成功了 ——
    // 而当时的 DELETE 恰恰是 `let _ =`，从没验证过。现在三件事同处一个事务，
    // 失败即整笔回滚，**没有半成品状态**，返回失败才是诚实的（客户端重试即可）。
    let counted = db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM activations WHERE card_id = $1")
            .bind(activation.card_id)
            .fetch_one(&mut *tx),
        "统计卡密剩余激活记录数",
    )
    .await;

    let remaining = match counted {
        db_guard::ScalarOutcome::Found(r) => r.0,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    if remaining == 0 {
        // 这一步也必须检查：写失败意味着卡密状态停在 active（界面上显示「已激活」），
        // 而这台设备其实已经解绑了 —— 一个没人会发现的不一致。
        if let Err(e) = sqlx::query(
            "UPDATE cards SET status = 'unused', activated_at = NULL, expires_at = NULL WHERE id = $1 AND status = 'active'",
        )
        .bind(activation.card_id)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(
                "解绑设备：恢复卡密状态失败: card_id={} activation_id={} err={}",
                activation.card_id,
                id,
                e
            );
            return db_guard::server_busy();
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("解绑设备：提交事务失败: activation_id={} err={}", id, e);
        return db_guard::server_busy();
    }

    Json(json!({"success": true, "message": "设备已解绑"}))
}
