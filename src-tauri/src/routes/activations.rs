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

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
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
    let card_code_raw = q.card_code.as_deref().unwrap_or("").trim().to_string();
    let card_code_filter = card_code_raw.to_lowercase();

    // 搜索路径：card_code 是加密存储的（cards.code_encrypted），SQL 无法 LIKE。
    //
    // 改造前这里**不分页**地拉出该商户的全部激活记录、逐行解密、内存 contains
    // 过滤、再内存分页。激活记录比卡密多（一张卡可能有多次设备绑定/解绑），
    // 所以这里的放大倍数比 cards 列表更高。
    //
    // 现在两层（与 cards.rs 的 list_cards 保持同一套策略）：
    //   ① 完整卡密 → `c.code_hash` 精确命中（唯一索引，代价与总量无关）
    //   ② 部分匹配 → 有界扫描，超过上限时返回 `truncated: true`
    if !card_code_filter.is_empty() {
        type Row = (Uuid, Uuid, String, Uuid, String, Option<String>, Option<String>, DateTime<Utc>, DateTime<Utc>);

        // ① 精确命中
        let mut exact_sql = String::from(
            r#"SELECT a.id, a.card_id, c.code_encrypted, a.app_id, a.device_id_encrypted,
                      a.device_name, a.ip_address, a.activated_at, a.last_verified_at
               FROM activations a
               JOIN cards c ON c.id = a.card_id
               WHERE c.merchant_id = $1 AND c.code_hash = $2
               ORDER BY a.activated_at DESC"#,
        );
        // 记录数上限同 cards：一张卡理论上可以有多次绑定记录，这里也封顶
        exact_sql.push_str(" LIMIT $3");

        let mut candidates: Vec<String> = vec![card_code_raw.clone()];
        let upper = card_code_raw.to_uppercase();
        if upper != card_code_raw {
            candidates.push(upper);
        }

        if card_code_raw.len() >= 6 {
            for cand in &candidates {
                let code_hash = EncryptedFieldsOps::generate_hash(cand);
                let rows: Result<Vec<Row>, sqlx::Error> = sqlx::query_as(&exact_sql)
                    .bind(merchant_id)
                    .bind(&code_hash)
                    .bind(crate::routes::cards::SEARCH_SCAN_LIMIT + 1)
                    .fetch_all(&state.pool)
                    .await;

                let rows = match rows {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("按哈希精确查询激活记录失败: err={}", e);
                        return db_guard::server_busy();
                    }
                };

                if rows.is_empty() {
                    continue; // 这种大小写没命中
                }

                let truncated = rows.len() as i64 > crate::routes::cards::SEARCH_SCAN_LIMIT;
                let mut matched = Vec::new();
                for (id, card_id, encrypted_code, app_id, encrypted_device_id, device_name, ip_address, activated_at, last_verified_at)
                    in rows.into_iter().take(crate::routes::cards::SEARCH_SCAN_LIMIT as usize)
                {
                    let card_code = EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &encrypted_code)
                        .unwrap_or_else(|_| "[解密失败]".to_string());
                    let device_id = EncryptedFieldsOps::decrypt_device_id(&state.encryptor, &encrypted_device_id)
                        .unwrap_or_else(|_| "[解密失败]".to_string());
                    matched.push(ActivationWithCode {
                        id, card_id, card_code, app_id, device_id,
                        device_name, ip_address, activated_at, last_verified_at,
                    });
                }

                return Json(json!({
                    "success": true,
                    "data": matched,
                    "total": matched.len(),
                    "page": 1,
                    "page_size": page_size,
                    "truncated": truncated,
                    "exact": true
                }));
            }
        }

        // ② 模糊匹配：有界扫描（多取 1 行用于判断是否被截断）
        let all: Vec<Row> = match sqlx::query_as(
            r#"SELECT a.id, a.card_id, c.code_encrypted, a.app_id, a.device_id_encrypted,
                      a.device_name, a.ip_address, a.activated_at, a.last_verified_at
               FROM activations a
               JOIN cards c ON c.id = a.card_id
               WHERE c.merchant_id = $1
               ORDER BY a.activated_at DESC
               LIMIT $2"#,
        )
        .bind(merchant_id)
        .bind(crate::routes::cards::SEARCH_SCAN_LIMIT + 1)
        .fetch_all(&state.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询激活记录列表失败: err={}", e);
                return db_guard::server_busy();
            }
        };

        let truncated = all.len() as i64 > crate::routes::cards::SEARCH_SCAN_LIMIT;

        let mut matched: Vec<ActivationWithCode> = Vec::new();
        for (id, card_id, encrypted_code, app_id, encrypted_device_id, device_name, ip_address, activated_at, last_verified_at)
            in all.into_iter().take(crate::routes::cards::SEARCH_SCAN_LIMIT as usize)
        {
            let card_code = EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &encrypted_code)
                .unwrap_or_else(|_| "[解密失败]".to_string());
            if !card_code.to_lowercase().contains(&card_code_filter) {
                continue;
            }
            let device_id = EncryptedFieldsOps::decrypt_device_id(&state.encryptor, &encrypted_device_id)
                .unwrap_or_else(|_| "[解密失败]".to_string());
            matched.push(ActivationWithCode {
                id, card_id, card_code, app_id, device_id,
                device_name, ip_address, activated_at, last_verified_at,
            });
        }

        let total = matched.len() as i64;
        let start = offset as usize;
        let end = (start + page_size as usize).min(matched.len());
        let page_acts = if start < matched.len() { matched[start..end].to_vec() } else { Vec::new() };

        return Json(json!({
            "success": true,
            "data": page_acts,
            "total": total,
            "page": page,
            "page_size": page_size,
            "truncated": truncated
        }));
    }

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
