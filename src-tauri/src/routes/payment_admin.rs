use crate::{
    middleware::auth::{admin_only, auth_middleware, AppState},
    models::payment_config::{PaymentConfig, PaymentConfigPublic, UpdatePaymentConfig},
    utils::db_guard,
};
use axum::{
    extract::{Path, Query, State},
    middleware,
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn payment_admin_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin/payment/configs", get(list_payment_configs))
        .route("/admin/payment/configs/:channel", get(get_payment_config))
        .route("/admin/payment/configs/:channel", patch(update_payment_config))
        .route("/admin/payment/configs/:channel/toggle", post(toggle_payment_config))
        .route("/admin/payment/orders", get(list_all_orders))
        .route_layer(middleware::from_fn(admin_only))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
}

fn config_public(c: &PaymentConfig) -> PaymentConfigPublic {
    PaymentConfigPublic {
        id: c.id,
        channel: c.channel.clone(),
        name: c.name.clone(),
        enabled: c.enabled,
        alipay_app_id_set: c.alipay_app_id.as_ref().map_or(false, |s| !s.is_empty()),
        xorpay_aid_set: c.xorpay_aid.as_ref().map_or(false, |s| !s.is_empty()),
        mbdpay_app_id_set: c.mbdpay_app_id.as_ref().map_or(false, |s| !s.is_empty()),
    }
}

async fn list_payment_configs(State(state): State<AppState>) -> Json<Value> {
    // ⚠️ 曾用 .unwrap_or_default()：查询失败时支付渠道列表变空，
    // 管理员看到「暂无支付渠道」以为支付配置全丢了，可能重新录入密钥；
    // 实际只是数据库故障，且这个页面本来也是收款链路配置的唯一视图。
    let configs: Vec<PaymentConfig> = match sqlx::query_as(
        "SELECT * FROM payment_configs ORDER BY channel",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询支付渠道配置列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let data: Vec<PaymentConfigPublic> = configs.iter().map(config_public).collect();

    Json(json!({
        "success": true,
        "data": data,
    }))
}

async fn get_payment_config(
    State(state): State<AppState>,
    Path(channel): Path<String>,
) -> Json<Value> {
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 前端显示「该渠道未配置」。
    // 管理员去配置页看到空的，会以为配置被清掉了，于是重新填一遍密钥 ——
    // 而真正的问题（数据库不可用）从没浮出来。
    let config = match db_guard::optional(
        sqlx::query_as::<_, PaymentConfig>("SELECT * FROM payment_configs WHERE channel = $1")
            .bind(&channel)
            .fetch_optional(&state.pool),
        "查询支付渠道配置",
    )
    .await
    {
        db_guard::QueryOutcome::Found(c) => Some(c),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    match config {
        Some(c) => Json(json!({
            "success": true,
            "data": {
                "id": c.id,
                "channel": c.channel,
                "name": c.name,
                "enabled": c.enabled,
                "xorpay_aid": c.xorpay_aid,
                "xorpay_app_key_set": c.xorpay_app_key.as_ref().map_or(false, |s| !s.is_empty()),
                "xorpay_notify_url": c.xorpay_notify_url,
                "mbdpay_app_id": c.mbdpay_app_id,
                "mbdpay_app_key_set": c.mbdpay_app_key.as_ref().map_or(false, |s| !s.is_empty()),
                "mbdpay_notify_url": c.mbdpay_notify_url,
                "alipay_app_id": c.alipay_app_id,
                "alipay_private_key_set": c.alipay_private_key.as_ref().map_or(false, |s| !s.is_empty()),
                "alipay_public_key_set": c.alipay_public_key.as_ref().map_or(false, |s| !s.is_empty()),
                "alipay_notify_url": c.alipay_notify_url,
                "alipay_gateway": c.alipay_gateway,
                "alipay_return_url": c.alipay_return_url,
            }
        })),
        None => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
    }
}

async fn update_payment_config(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    Json(body): Json<UpdatePaymentConfig>,
) -> Json<Value> {
    let mut updates: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    macro_rules! add_update {
        ($field:literal, $value:expr) => {
            if $value.is_some() {
                updates.push(format!("{} = ${}", $field, params.len() + 1));
                params.push($value.unwrap());
            }
        };
    }

    add_update!("name", body.name);
    add_update!("xorpay_aid", body.xorpay_aid);
    add_update!("xorpay_app_key", body.xorpay_app_key);
    add_update!("xorpay_notify_url", body.xorpay_notify_url);
    add_update!("mbdpay_app_id", body.mbdpay_app_id);
    add_update!("mbdpay_app_key", body.mbdpay_app_key);
    add_update!("mbdpay_notify_url", body.mbdpay_notify_url);
    add_update!("alipay_app_id", body.alipay_app_id);
    add_update!("alipay_private_key", body.alipay_private_key);
    add_update!("alipay_public_key", body.alipay_public_key);
    add_update!("alipay_notify_url", body.alipay_notify_url);
    add_update!("alipay_gateway", body.alipay_gateway);
    add_update!("alipay_return_url", body.alipay_return_url);

    if updates.is_empty() {
        return Json(json!({
            "success": false,
            "message": "没有需要更新的字段"
        }));
    }

    updates.push("updated_at = NOW()".to_string());

    let query = format!(
        "UPDATE payment_configs SET {} WHERE channel = ${}",
        updates.join(", "),
        params.len() + 1
    );

    let mut q = sqlx::query(&query);
    for p in &params {
        q = q.bind(p);
    }
    q = q.bind(&channel);
    let result = q.execute(&state.pool).await;

    state.invalidate_payment_cache(Some(&channel)).await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "message": "配置已更新"
            }))
        }
        Ok(_) => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
        Err(e) => db_guard::internal_error("更新支付渠道配置", e),
    }
}

async fn toggle_payment_config(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let enabled = match body.get("enabled").and_then(|v| v.as_bool()) {
        Some(v) => v,
        None => return Json(json!({
            "success": false,
            "message": "缺少 enabled 参数"
        })),
    };

    if enabled {
        // ── 启用：这两条 UPDATE 必须**同生同死** ──────────────────────────
        //
        // 「单选模式」是这张表的不变量：任何时刻最多一个渠道 enabled，
        // 它靠「先全禁用，再启用目标」这两条语句共同维护。
        // 原来的写法没有事务，于是有两个洞：
        //
        // 1. 第一条的失败被 `.ok()` 吞掉 → 「全禁用」没生效，
        //    第二条照样把目标置为 enabled → **两个渠道同时生效**，
        //    接口却返回「已启用」。这不是「多一个可选项」那么轻：
        //    下单时用哪个通道取决于调用方拼的 channel 参数，
        //    「哪个通道真的在收钱」变得不确定。
        // 2. `channel` 不存在时更隐蔽：第一条**成功**禁用了全部渠道，
        //    第二条影响 0 行 → 返回「配置不存在」，但此刻**所有渠道都已被禁用**，
        //    线上支付静默全灭，管理员看到的只是一句无关痛痒的提示。
        //
        // 放进一个事务，两个洞一起消失：要么完整地变，要么完整地不变。
        let mut tx = match state.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                return db_guard::internal_error("支付渠道操作", e)
            }
        };

        if let Err(e) = sqlx::query("UPDATE payment_configs SET enabled = FALSE")
            .execute(&mut *tx)
            .await
        {
            // 不 commit → tx 在作用域结束时自动回滚
            tracing::error!("启用支付渠道前禁用其他渠道失败，已回滚: channel={} err={}", channel, e);
            return db_guard::internal_error("支付渠道操作", e);
        }

        let activated = sqlx::query(
            "UPDATE payment_configs SET enabled = TRUE, updated_at = NOW() WHERE channel = $1",
        )
        .bind(&channel)
        .execute(&mut *tx)
        .await;

        let rows = match activated {
            Ok(r) => r.rows_affected(),
            Err(e) => {
                tracing::error!("启用支付渠道失败，已回滚: channel={} err={}", channel, e);
                return db_guard::internal_error("支付渠道操作", e);
            }
        };

        if rows == 0 {
            // 目标渠道不存在 → 不 commit，前面那条「禁用所有」随之回滚
            return Json(json!({
                "success": false,
                "message": "配置不存在"
            }));
        }

        if let Err(e) = tx.commit().await {
            tracing::error!("启用支付渠道提交失败: channel={} err={}", channel, e);
            return db_guard::internal_error("支付渠道操作", e);
        }

        state.invalidate_payment_cache(None).await;

        return Json(json!({
            "success": true,
            "message": "已启用"
        }));
    }

    // ── 禁用：单条 UPDATE，本身就是原子的 ──
    let result = sqlx::query(
        "UPDATE payment_configs SET enabled = $1, updated_at = NOW() WHERE channel = $2",
    )
    .bind(false)
    .bind(&channel)
    .execute(&state.pool)
    .await;

    // 缓存只在**确实改动过**之后失效：失败和「配置不存在」都没有改库，
    // 缓存里的内容仍然是对的，清掉反而多一次无谓的加载。
    match result {
        Ok(r) if r.rows_affected() > 0 => {
            state.invalidate_payment_cache(None).await;
            Json(json!({
                "success": true,
                "message": "已禁用"
            }))
        }
        Ok(_) => Json(json!({
            "success": false,
            "message": "配置不存在"
        })),
        Err(e) => db_guard::internal_error("支付渠道操作", e),
    }
}

// ── 订单管理 ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AdminOrdersQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub status: Option<String>,    // 筛选：paid / pending / expired / cancelled
    pub channel: Option<String>,   // 筛选：alipay / xorpay / mbdpay
}

type AdminOrderRow = (
    String,             // order_id
    String,             // merchant_id
    String,             // username
    String,             // pay_channel
    String,             // pay_type
    String,             // amount::text
    String,             // status
    Option<i32>,        // expires_days
    chrono::DateTime<chrono::Utc>, // created_at
    Option<chrono::DateTime<chrono::Utc>>, // pay_time
    Option<String>,     // pay_price::text
);

async fn list_all_orders(
    State(state): State<AppState>,
    Query(q): Query<AdminOrdersQuery>,
) -> Json<Value> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // ⚠️ 曾用 `.unwrap_or_default()` + `.unwrap_or((0,))`：管理端的订单总览
    // 静默变成「一笔订单都没有」——运维据此判断「今天没人下单」，
    // 这个结论是反的，而且数字（0）看起来完全合理。
    let orders: Vec<AdminOrderRow> = match sqlx::query_as(
        r#"
        SELECT p.order_id, p.merchant_id::text, COALESCE(m.username, '(已删除)') AS username,
               p.pay_channel, p.pay_type, p.amount::text, p.status,
               p.expires_days, p.created_at, p.pay_time, p.pay_price::text
        FROM payments p
        LEFT JOIN merchants m ON m.id = p.merchant_id
        WHERE ($1::text IS NULL OR p.status = $1)
          AND ($2::text IS NULL OR p.pay_channel = $2)
        ORDER BY p.created_at DESC
        LIMIT $3 OFFSET $4
        "#
    )
    .bind(&q.status)
    .bind(&q.channel)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询管理端订单列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM payments WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR pay_channel = $2)"
        )
        .bind(&q.status)
        .bind(&q.channel)
        .fetch_one(&state.pool),
        "统计管理端订单总数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(t) => t,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    let data: Vec<Value> = orders.into_iter().map(|(
        order_id, merchant_id, username, pay_channel, pay_type,
        amount, status, expires_days, created_at, pay_time, pay_price,
    )| {
        json!({
            "order_id": order_id,
            "merchant_id": merchant_id,
            "username": username,
            "pay_channel": pay_channel,
            "pay_type": pay_type,
            "amount": amount,
            "status": status,
            "expires_days": expires_days,
            "created_at": created_at.to_rfc3339(),
            "pay_time": pay_time.map(|t| t.to_rfc3339()),
            "pay_price": pay_price,
        })
    }).collect();

    Json(json!({
        "success": true,
        "data": data,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}
