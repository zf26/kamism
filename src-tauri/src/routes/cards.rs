use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::auth::{AppState, auth_middleware},
    models::card::Card,
    routes::plan_config::get_config_by_plan,
    utils::{card_gen::generate_card_code_with_format, db_guard, jwt::Claims, kms::Encryptor},
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    middleware,
    response::Response,
    routing::{get, patch, post},
    BoxError, Extension, Json, Router,
};
use axum::http::{header, StatusCode};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct GenerateCardsRequest {
    pub app_id: Uuid,
    pub count: u32,
    pub duration_days: i32,
    pub max_devices: i32,
    pub note: Option<String>,
    /// 卡密前缀，默认 "KAMI"，最长 16 字符，仅限字母数字
    pub prefix: Option<String>,
    /// 段数（不含前缀），1-8，默认 4
    pub segment_count: Option<usize>,
    /// 每段字符数，2-8，默认 4
    pub segment_len: Option<usize>,
}

#[derive(Deserialize)]
pub struct BatchExtendRequest {
    pub ids: Vec<Uuid>,
    /// 正数延期，负数缩短，单位：天
    pub days: i32,
}

// ── 代理配额判定 ────────────────────────────────────────────────────────────
//
// 抽成纯函数是为了让「口径」这件事能被单元测试钉住。
// 之前这个判定内联在 400 行的 handler 里，口径写错了没有任何测试会红。
//
// 语义（与 README 和前端一致）：配额 = 允许**生成**的卡密张数。

/// 一条代理关系的配额判定结果
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QuotaVerdict {
    /// 允许本次生成
    Allow,
    /// 拒绝，附带给用户的消息
    Deny(String),
}

/// 判定「已生成 `used` 张、本次要再生成 `need` 张」是否在 `quota_total` 之内。
///
/// - `quota_total < 0` → 不限制
/// - `quota_total == 0` → 一张都不许生成（这是**有意**的，不是"未设置"。
///   未设置的情况由 `create_invite` 的 `quota_total.unwrap_or(0).max(0)` 决定，
///   语义上就是 0 张额度。）
pub(crate) fn judge_agent_quota(quota_total: i32, used: i64, need: i64) -> QuotaVerdict {
    if quota_total < 0 {
        return QuotaVerdict::Allow;
    }
    let total = quota_total as i64;
    if used + need > total {
        let remaining = (total - used).max(0);
        return QuotaVerdict::Deny(format!(
            "代理配额不足：配额 {}，已用 {}，剩余 {}，本次需要 {}",
            quota_total, used, remaining, need
        ));
    }
    QuotaVerdict::Allow
}

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct CardGroupStat {
    pub duration_days: i32,
    pub max_devices: i32,
    pub total: i64,
    pub unused: i64,
    pub active: i64,
    pub expired: i64,
    pub disabled: i64,
}

#[derive(Deserialize)]
pub struct ExtendCardRequest {
    pub days: i32,
}

#[derive(Deserialize)]
pub struct UpdateNoteRequest {
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct CardQuery {
    pub app_id: Option<Uuid>,
    pub status: Option<String>,
    /// 按卡密代码**子串**搜索（大小写不敏感）。
    /// 卡密 code 是加密存储的（`code_encrypted`），SQL 里没法 LIKE ——
    /// 所以带这个参数时走「全量解密 + 内存过滤 + 内存分页」路径（见 list_cards）。
    pub card_code: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

/// 卡密「激活与否」过滤的归一化结果。
///
/// 为什么不是直接把 `status` 字符串拼进 SQL：
///   卡密的 `status='expired'` 是**懒标记**的 —— 只有「过期后再次尝试激活」才会
///   把它 UPDATE 成 expired（见 public_api.rs）。一张激活后到期、但没再被激活过
///   的卡，status 仍是 `active`，只是 `expires_at` 已经过去了。
///   所以「已过期」必须实时按 `expires_at <= NOW()` 判断，而不是 `status='expired'`，
///   否则这类卡会从「已过期」过滤里漏掉。
///
/// 三分类（互斥、无重叠）：
///   - 未激活：从没激活过（status='unused'）
///   - 已激活：激活了且还没到期（status='active' 且 expires_at 为空或未到）
///   - 已过期：激活过但已经到期（expires_at 不为空且已过，不管 status 是 active 还是 expired）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardStatusFilter {
    Unused,
    Active,
    Expired,
}

impl CardStatusFilter {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "unused" => Some(Self::Unused),
            "active" => Some(Self::Active),
            "expired" => Some(Self::Expired),
            _ => None, // 非法值 → None，调用方决定「不过滤」还是「报参数错误」
        }
    }

    /// 生成对应的 WHERE 条件片段（不含前导 AND，调用方自己接）。
    /// 片段内**没有**绑定参数，都是字面量，可安全拼进静态 SQL。
    fn sql_fragment(self) -> &'static str {
        match self {
            Self::Unused => "status = 'unused'",
            Self::Active => "status = 'active' AND (expires_at IS NULL OR expires_at > NOW())",
            Self::Expired => "expires_at IS NOT NULL AND expires_at <= NOW()",
        }
    }
}

#[derive(Deserialize)]
pub struct BatchCardStatusRequest {
    pub ids: Vec<Uuid>,
    /// "disabled" 或 "unused"（启用）
    pub action: String,
}

pub fn cards_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/cards", get(list_cards).post(generate_cards))
        .route("/cards/export", get(export_cards_csv))
        .route("/cards/batch-status", post(batch_update_card_status))
        .route("/cards/batch-extend", post(batch_extend_cards))
        .route("/cards/stats", get(card_group_stats))
        .route("/cards/:id", get(get_card).delete(delete_card))
        .route("/cards/:id/disable", patch(disable_card))
        .route("/cards/:id/enable", patch(enable_card))
        .route("/cards/:id/extend", patch(extend_card))
        .route("/cards/:id/note", patch(update_card_note))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware))
}

fn merchant_id_from_claims(claims: &Claims) -> Result<Uuid, Json<Value>> {
    Uuid::parse_str(&claims.sub)
        .map_err(|_| Json(json!({"success": false, "message": "无效用户ID"})))
}

async fn list_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<CardQuery>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    const CARD_COLS: &str = "id, app_id, merchant_id, code_encrypted, duration_days, max_devices, status, note, created_at, activated_at, expires_at";

    #[derive(sqlx::FromRow)]
    struct CardWithTotal {
        id: Uuid,
        app_id: Uuid,
        merchant_id: Uuid,
        #[sqlx(rename = "code_encrypted")]
        code: String,
        duration_days: i32,
        max_devices: i32,
        status: String,
        note: Option<String>,
        created_at: chrono::DateTime<chrono::Utc>,
        activated_at: Option<chrono::DateTime<chrono::Utc>>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        total_count: i64,
    }

    // status 过滤：归一化成枚举后取字面量条件片段（见 CardStatusFilter 注释）。
    // 非法 status 值 → 视为「不过滤」（返回全部），与 app_id 的宽松语义一致；
    // 枚举保证片段只可能是三个受控字符串，无注入面。
    let status_filter = q.status.as_deref().and_then(CardStatusFilter::parse);

    // 卡密代码搜索词（trim + 小写，供解密后 contains 匹配）。
    let card_code_filter = q.card_code.as_deref().unwrap_or("").trim().to_lowercase();

    // ── 搜索路径：code 是加密存储的，SQL 无法 LIKE ─────────────────────────
    // 先按 app_id/status 把该商户的卡**全量**拉出来（不分页），解密后在内存里
    // contains 过滤，再做内存分页。这样搜索结果是**全量**的，而不是「只搜当前页」。
    // 代价是全量解密（单商户卡密量级通常几百到几千，毫秒级，可接受）。
    if !card_code_filter.is_empty() {
        #[derive(sqlx::FromRow)]
        struct CardRow {
            id: Uuid,
            app_id: Uuid,
            merchant_id: Uuid,
            #[sqlx(rename = "code_encrypted")]
            code: String,
            duration_days: i32,
            max_devices: i32,
            status: String,
            note: Option<String>,
            created_at: chrono::DateTime<chrono::Utc>,
            activated_at: Option<chrono::DateTime<chrono::Utc>>,
            expires_at: Option<chrono::DateTime<chrono::Utc>>,
        }

        let mut filter_sql = format!("SELECT {} FROM cards WHERE merchant_id = $1", CARD_COLS);
        if q.app_id.is_some() {
            filter_sql.push_str(" AND app_id = $2");
        }
        if let Some(f) = status_filter {
            filter_sql.push_str(" AND ");
            filter_sql.push_str(f.sql_fragment());
        }
        filter_sql.push_str(" ORDER BY created_at DESC");

        let mut fq = sqlx::query_as::<_, CardRow>(&filter_sql).bind(merchant_id);
        if q.app_id.is_some() {
            fq = fq.bind(q.app_id.unwrap());
        }
        let all: Vec<CardRow> = match fq.fetch_all(&state.pool).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询卡密列表失败: err={}", e);
                return db_guard::server_busy();
            }
        };

        // 解密 + contains 过滤
        let mut matched: Vec<Card> = Vec::new();
        for r in all {
            let plain = match EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &r.code) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("解密卡密 {} 失败: {}", r.id, e);
                    continue; // 解不开的卡无法匹配搜索词，跳过
                }
            };
            if !plain.to_lowercase().contains(&card_code_filter) {
                continue;
            }
            matched.push(Card {
                id: r.id, app_id: r.app_id, merchant_id: r.merchant_id,
                code: plain, duration_days: r.duration_days, max_devices: r.max_devices,
                status: r.status, note: r.note, created_at: r.created_at,
                activated_at: r.activated_at, expires_at: r.expires_at,
            });
        }

        let total = matched.len() as i64;
        let start = offset as usize;
        let end = (start + page_size as usize).min(matched.len());
        let page_cards = if start < matched.len() {
            matched[start..end].to_vec()
        } else {
            Vec::new()
        };

        return Json(json!({
            "success": true,
            "data": page_cards,
            "total": total,
            "page": page,
            "page_size": page_size
        }));
    }

    // ── 非搜索路径：SQL 分页（带 COUNT(*) OVER()）──────────────────────────
    let mut query_sql = format!(
        "SELECT {}, COUNT(*) OVER() AS total_count FROM cards WHERE merchant_id = $1",
        CARD_COLS
    );
    if q.app_id.is_some() {
        query_sql.push_str(" AND app_id = $4");
    }
    if let Some(f) = status_filter {
        query_sql.push_str(" AND ");
        query_sql.push_str(f.sql_fragment());
    }
    query_sql.push_str(" ORDER BY created_at DESC LIMIT $2 OFFSET $3");

    let mut row_query = sqlx::query_as::<_, CardWithTotal>(&query_sql)
        .bind(merchant_id)
        .bind(page_size)
        .bind(offset);

    if q.app_id.is_some() {
        row_query = row_query.bind(q.app_id.unwrap());
    }

    // ⚠️ 曾用 .unwrap_or_default()：数据库故障被压成空列表，卡密列表页
    // 会显示「暂无卡密」且 total=0，商户看到一片空白以为卡密全丢了，
    // 而日志里一个字都没有 —— 与「这个商户真的没生成过卡密」完全无法区分。
    let rows: Vec<CardWithTotal> = match row_query.fetch_all(&state.pool).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询卡密列表失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    let total = rows.first().map(|r| r.total_count).unwrap_or(0);
    let mut cards: Vec<Card> = rows.into_iter().map(|r| Card {
        id: r.id, app_id: r.app_id, merchant_id: r.merchant_id,
        code: r.code, duration_days: r.duration_days, max_devices: r.max_devices,
        status: r.status, note: r.note, created_at: r.created_at,
        activated_at: r.activated_at, expires_at: r.expires_at,
    }).collect();

    for card in &mut cards {
        match EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &card.code) {
            Ok(plain) => card.code = plain,
            Err(e) => tracing::warn!("解密卡密 {} 失败: {}", card.id, e),
        }
    }

    Json(json!({
        "success": true,
        "data": cards,
        "total": total,
        "page": page,
        "page_size": page_size
    }))
}

async fn export_cards_csv(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<CardQuery>,
) -> Result<Response<Body>, StatusCode> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    // 每批取多少行。5000 行 × 每行几十字节 ≈ 几百 KB 一个 chunk，
    // 内存占用近似常数，几十万张卡密也不会把内存打满。
    const BATCH: i64 = 5000;

    // ── 预检：在发送响应头之前先确认「能查」 ─────────────────────────────
    //
    // 流式响应一旦开始（返回 Response），中途出错就没法再改状态码了 ——
    // 那时只能提前截断，商户会拿到一份「看起来完整、其实缺了后半」的 CSV。
    // 所以把「硬错误」（连接断、SQL 不可执行）尽量在**发头前**拦住：
    // 这里用同一条 WHERE 做一次 LIMIT 1 的探测，能通过才往下走。
    // 探测本身不取业务数据，开销是一次索引扫描。
    {
        let mut probe = sqlx::QueryBuilder::new("SELECT 1 FROM cards WHERE merchant_id = ");
        probe.push_bind(merchant_id);
        if let Some(app) = q.app_id {
            probe.push(" AND app_id = ").push_bind(app);
        }
        if let Some(ref status) = q.status {
            probe.push(" AND status = ").push_bind(status);
        }
        probe.push(" LIMIT 1");
        if let Err(e) = probe.build().fetch_optional(&state.pool).await {
            tracing::error!("导出卡密预检失败: err={}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    // ── 流式导出 ───────────────────────────────────────────────────────────
    //
    // 为什么改成流式（而不是原来的一次 `fetch_all` 全表进内存）：
    //   原实现 `Vec<Card>` + 拼一个完整 `String`，商户卡密一多（几万张），
    //   内存峰值就是「全表 Card + 全量 CSV 字符串」两倍，很容易 OOM。
    //   流式后内存近似 O(1)，与卡密总量无关。
    //
    // 同时把 status 过滤**下推到 SQL**（原来是在内存里 `retain`）：
    //   这样导出「已禁用」这类子集时，不会先把全表读进内存再丢掉大部分。
    //
    // 错误语义的取舍（如实说明）：
    //   预检通过后的中途失败（导出过程中数据库抖动）只能**提前终止 stream**，
    //   此时商户会拿到不完整的 CSV。代价是「一次导出可能不完整」，
    //   换来的是「不会把服务端内存打满」。这个取舍对导出场景是划算的，
    //   而且中断会留下 error 日志，不会静默。
    let pool = state.pool.clone();
    let encryptor = Arc::new(state.encryptor);
    let app_id = q.app_id;
    let status = q.status.clone();
    // 与 list_cards 一致：把 status 归一化成枚举，取受控的字面量条件片段，
    // 使「已过期」在导出与列表里的口径相同（都是实时按 expires_at 判断）。
    let status_filter = status.as_deref().and_then(CardStatusFilter::parse);

    // 表头 + BOM 单独作为 stream 的第一个 chunk
    let header = futures_util::stream::once(async move {
        Ok::<String, BoxError>(
            "\u{feff}卡密代码,有效天数,设备上限,状态,创建时间,激活时间,过期时间,备注\n".to_string(),
        )
    });

    let rows = futures_util::stream::try_unfold(0i64, move |offset| {
        let pool = pool.clone();
        let encryptor = Arc::clone(&encryptor);
        let app_id = app_id;
        let status_filter = status_filter;
        async move {
            let mut qb = sqlx::QueryBuilder::new("SELECT * FROM cards WHERE merchant_id = ");
            qb.push_bind(merchant_id);
            if let Some(app) = app_id {
                qb.push(" AND app_id = ").push_bind(app);
            }
            if let Some(f) = status_filter {
                qb.push(" AND ").push(f.sql_fragment());
            }
            qb.push(" ORDER BY created_at DESC LIMIT ")
                .push_bind(BATCH)
                .push(" OFFSET ")
                .push_bind(offset);

            let batch: Vec<Card> = qb
                .build_query_as::<Card>()
                .fetch_all(&pool)
                .await
                .map_err(|e| -> BoxError { Box::new(e) })?;

            if batch.is_empty() {
                return Ok(None);
            }

            let chunk = cards_to_csv_chunk(batch, &encryptor);
            Ok(Some((chunk, offset + BATCH)))
        }
    });

    let body_stream = header.chain(rows);
    let body = Body::from_stream(body_stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"cards.csv\"")
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// 把一批卡密解密后拼成 CSV 行（不含表头）。
///
/// ⚠️ 解密失败的卡密：保留**密文**并 `warn!`，而不是静默 ——
/// 早期版本解密失败时静默保留密文，商户拿到 CSV 后看到「代码」列是一串
/// 乱码，会以为是导出坏了，但日志里一个字都没有，无从排查。
fn cards_to_csv_chunk(cards: Vec<Card>, encryptor: &Encryptor) -> String {
    let mut csv = String::with_capacity(cards.len() * 64);
    for mut c in cards {
        match EncryptedFieldsOps::decrypt_card_code(encryptor, &c.code) {
            Ok(plain) => c.code = plain,
            Err(e) => tracing::warn!("导出时解密卡密 {} 失败，保留密文: {}", c.id, e),
        }
        let status_label = match c.status.as_str() {
            "unused" => "未使用",
            "active" => "使用中",
            "expired" => "已过期",
            "disabled" => "已禁用",
            _ => c.status.as_str(),
        };
        let activated = c.activated_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let expires = c.expires_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let note = c.note.as_deref().unwrap_or("").replace(',', "，");
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            c.code, c.duration_days, c.max_devices, status_label,
            c.created_at.format("%Y-%m-%d %H:%M"),
            activated, expires, note,
        ));
    }
    csv
}

async fn generate_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<GenerateCardsRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    if body.count == 0 || body.count > 1000 {
        return Json(json!({"success": false, "message": "生成数量需在 1-1000 之间"}));
    }
    if body.duration_days <= 0 {
        return Json(json!({"success": false, "message": "有效天数必须大于0"}));
    }
    if body.max_devices <= 0 || body.max_devices > 100 {
        return Json(json!({"success": false, "message": "设备数量需在 1-100 之间"}));
    }

    // 查询失败时按免费版处理：方向是安全的（免费版的四项配额都比专业版严格 ——
    // free: 1 应用 / 500 卡密 / 3 设备 / 单次 100；pro: -1 / -1 / 100 / 1000），
    // 所以这个降级**不会放宽**任何限制。但**必须留痕**：
    // 不留痕的话，专业版商户看到的是「免费版最多拥有 500 张卡密，请升级套餐」——
    // 一句完整的业务话术，而真相是数据库故障。排查方向被彻底带偏。
    // （`apps.rs::create_app` 是同一个坑的另一半，两处必须保持一致。）
    let plan: (String,) = match sqlx::query_as("SELECT plan FROM merchants WHERE id = $1")
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "查询商户套餐失败，本次卡密配额检查按免费版处理: merchant_id={} err={}",
                merchant_id,
                e
            );
            ("free".to_string(),)
        }
    };

    let config = get_config_by_plan(&state.pool, &plan.0).await;

    if config.max_gen_once != -1 && body.count > config.max_gen_once as u32 {
        return Json(json!({
            "success": false,
            "message": format!("{}单次最多生成 {} 张卡密", config.label, config.max_gen_once)
        }));
    }
    if config.max_cards != -1 {
        // ⚠️⚠️ 与 `apps.rs` 的配额检查同源问题，但后果更直接 ——
        // 这是**生成卡密**的接口，而卡密就是钱。
        //
        // 查询失败 → 已有卡密数当成 0 → `0 + 本次数量 > max_cards` 为假 →
        // **跳过配额检查** → 免费用户（`max_cards = 500`）可以无限批量出卡。
        // 一次请求 `body.count` 就能生成成百上千张，且整个过程没有任何日志。
        let card_count = match db_guard::scalar(
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM cards WHERE merchant_id = $1")
                .bind(merchant_id)
                .fetch_one(&state.pool),
            "统计已有卡密数（配额检查）",
        )
        .await
        {
            db_guard::ScalarOutcome::Found(c) => c,
            db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
        };
        if card_count.0 + body.count as i64 > config.max_cards as i64 {
            return Json(json!({
                "success": false,
                "message": format!("{}最多拥有 {} 张卡密（当前已有 {} 张），请升级套餐", config.label, config.max_cards, card_count.0)
            }));
        }
    }
    if config.max_devices != -1 && body.max_devices > config.max_devices {
        return Json(json!({
            "success": false,
            "message": format!("{}单张卡密最多绑定 {} 台设备，请升级套餐", config.label, config.max_devices)
        }));
    }

    // ── 代理配额校验 ────────────────────────────────────────────────────────
    //
    // 配额语义 = 「这个代理被允许**生成**多少张卡密」（README:48/72/410 与前端
    // 「已用 / 配额」进度条都是这个口径）。
    //
    // ⚠️ 早期实现的 bug：判定写成 `quota_used + body.count > quota_total`，
    // 而 `quota_used` 只在 `agent::record_commission`（激活时）里 +1。
    // 也就是说：**拿"激活次数"去和"生成张数"相加比较**。后果是
    //   - 代理只生成、不激活 → quota_used 永远是 0 → 配额完全不起作用，可以无限生成
    //   - 代理生成后全部激活 → quota_used 反超 quota_total，
    //     此时 `update_quota` 想把配额调小会被 `new_total < quota_used` 无理由拦住
    //
    // 现在改为直接用「该代理名下的卡密总数」作为已用量，
    // 与 `quota_total` 同量纲。`quota_used` 字段不再独立累加，
    // 统一从 cards 表派生（见 agent.rs 的读取点），杜绝两处口径漂移。
    let rel: Option<(Uuid, i32)> = match sqlx::query_as(
        "SELECT id, quota_total FROM agent_relations
         WHERE agent_id = $1 AND agent_id != parent_id AND status = 'active' LIMIT 1",
    )
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // 不能 `unwrap_or(None)`：数据库出错时静默降级 = 配额校验被绕过，
            // 而调用方会拿到「生成成功」，谁都不知道配额失控了。
            tracing::error!("查询代理配额失败: merchant_id={} err={}", merchant_id, e);
            return Json(json!({"success": false, "message": "服务器繁忙，请稍后重试"}));
        }
    };

    if let Some((rel_id, quota_total)) = rel {
        // quota_total < 0 表示不限制（与套餐里 -1 = unlimited 的约定一致）
        if quota_total >= 0 {
            let used: (i64,) = match sqlx::query_as(
                "SELECT COUNT(*) FROM cards WHERE merchant_id = $1",
            )
            .bind(merchant_id)
            .fetch_one(&state.pool)
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("统计代理已用配额失败: relation_id={} err={}", rel_id, e);
                    return Json(json!({"success": false, "message": "服务器繁忙，请稍后重试"}));
                }
            };

            if let QuotaVerdict::Deny(msg) = judge_agent_quota(quota_total, used.0, body.count as i64)
            {
                return Json(json!({"success": false, "message": msg}));
            }
        }
    }

    // 验证 app 归属
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「应用不存在或已禁用」。
    // 收严方向（不会误放行），但把「库抖动」说成了「你的应用被禁用了」——
    // 用户第一反应是去检查应用状态，而问题根本不在那儿。
    let app_exists = match db_guard::optional(
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM apps WHERE id = $1 AND merchant_id = $2 AND status = 'active'")
            .bind(body.app_id)
            .bind(merchant_id)
            .fetch_optional(&state.pool),
        "校验应用归属（生成卡密）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if app_exists.is_none() {
        return Json(json!({"success": false, "message": "应用不存在或已禁用"}));
    }

    let prefix = body.prefix.as_deref().unwrap_or("KAMI");
    if prefix.len() > 16 || !prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Json(json!({"success": false, "message": "前缀最长 16 字符，仅限字母、数字、- 和 _"}));
    }
    let seg_count = body.segment_count.unwrap_or(4).clamp(1, 8);
    let seg_len   = body.segment_len.unwrap_or(4).clamp(2, 8);

    let codes: Vec<String> = (0..body.count)
        .map(|_| generate_card_code_with_format(Some(prefix), seg_count, seg_len))
        .collect();

    // 卡密 id 在这里就定下来，不再依赖 `INSERT ... RETURNING` 回填。
    //
    // 为什么：加密日志要记「这张卡密是用哪个 key_id 加密的」，而 key_id 里嵌着
    // 记录 id（`card_code_{id}`，与 merchants / activations 的约定一致）。
    // 以前是先生成一个临时 uuid 去加密、INSERT 之后再拿数据库生成的 id 回填日志，
    // 于是日志里出现 `record_id = <真 id>` 而 `key_id = card_code_<临时 id>` ——
    // 两个 id 对不上，读日志的人会以为这条记录被改过。
    // 直接在 Rust 侧生成 id，两边就是同一个值，也省掉一次 zip 回填。
    let mut encrypted_codes = Vec::with_capacity(codes.len());
    for code in &codes {
        let card_id = Uuid::new_v4();
        let key_id = format!("card_code_{}", card_id);
        let code_hash = EncryptedFieldsOps::generate_hash(code);
        match state.encryptor.encrypt(code, &key_id) {
            Ok(encrypted) => encrypted_codes.push((card_id, encrypted, code_hash)),
            Err(e) => return db_guard::internal_error("加密卡密", e),
        }
    }

    let mut params_sql = String::new();
    // 列顺序：id, app_id, merchant_id, code_encrypted, code_hash, duration_days, max_devices, note
    let base = 8usize;
    for i in 0..encrypted_codes.len() {
        let n = i * base;
        if i > 0 { params_sql.push(','); }
        params_sql.push_str(&format!("(${},${},${},${},${},${},${},${})", n+1, n+2, n+3, n+4, n+5, n+6, n+7, n+8));
    }
    let sql = format!(
        "INSERT INTO cards (id, app_id, merchant_id, code_encrypted, code_hash, duration_days, max_devices, note) VALUES {}",
        params_sql
    );

    let mut q = sqlx::query(&sql);
    for (card_id, encrypted_code, code_hash) in &encrypted_codes {
        q = q
            .bind(card_id)
            .bind(body.app_id)
            .bind(merchant_id)
            .bind(encrypted_code)
            .bind(code_hash)
            .bind(body.duration_days)
            .bind(body.max_devices)
            .bind(&body.note);
    }

    // ── 卡密 INSERT + 加密日志：同一个事务 ─────────────────────────────────
    //
    // ⚠️ 这里以前是「先 INSERT（自动提交），再 `tokio::spawn` 一个游离任务去写
    // `encrypted_fields_log`」。三个问题，按严重程度排：
    //
    //   1. **日志可能永远写不上，而且没人知道**：游离任务与请求生命周期无关，
    //      写失败只留一行 `error!`；进程退出 / 容器重建时任务被直接丢掉。
    //      结果是「卡密在、日志缺」，而这张日志表正是按行反查 key_id 的唯一索引。
    //   2. **孤儿日志**：反方向也成立（事务外写入不随业务回滚）。
    //      实测库里 139 条日志**全部**是孤儿，见 `migrations/010_...` 的推导。
    //   3. 多发一条池连接：主写入已提交，这里再从池里拿一条 ——
    //      「连接池自耗」是同一类问题（旧 `log_encryption` 的注释里写过）。
    //
    // 现在日志和卡密同生共死：日志写不上就整批回滚，宁可让用户重试，
    // 也不留下一批「日志对不上」的卡密。commit 失败同理。
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("生成卡密时开启事务失败: merchant_id={} err={}", merchant_id, e);
            return Json(json!({"success": false, "message": "服务器繁忙，请稍后重试"}));
        }
    };

    let inserted = match q.execute(&mut *tx).await {
        Ok(r) => r,
        Err(e) => {
            // 显式 rollback 只是为了立刻释放连接；即使漏了，`tx` 析构也会回滚
            tracing::error!("写入卡密失败: merchant_id={} err={}", merchant_id, e);
            return db_guard::internal_error("生成卡密", e);
        }
    };

    // 一次 INSERT 多行：行数不符说明 SQL 拼错了。不能放过 ——
    // 「少插了几行但接口回 200」是用户最难自己发现的一类问题。
    if inserted.rows_affected() as usize != encrypted_codes.len() {
        tracing::error!(
            "卡密写入行数与预期不符，整批回滚: 预期={} 实际={}",
            encrypted_codes.len(),
            inserted.rows_affected()
        );
        return Json(json!({"success": false, "message": "生成失败，请稍后重试"}));
    }

    for (card_id, _, _) in &encrypted_codes {
        let key_id = format!("card_code_{}", card_id);
        if let Err(e) =
            EncryptedFieldsOps::log_encryption_tx(&mut *tx, "cards", *card_id, "code", &key_id).await
        {
            // 不吞掉：卡密已经在这个事务里了，回滚即可。
            // 绝不能「日志写不上就照常提交」—— 那就又回到静默的半成品状态。
            tracing::error!("写入卡密加密日志失败，整批回滚: card_id={} err={}", card_id, e);
            return Json(json!({"success": false, "message": "生成失败，请稍后重试"}));
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("提交生成卡密事务失败: merchant_id={} err={}", merchant_id, e);
        return Json(json!({"success": false, "message": "生成失败，请稍后重试"}));
    }

    Json(json!({
        "success": true,
        "message": format!("成功生成 {} 张卡密", encrypted_codes.len()),
        "count": encrypted_codes.len()
    }))
}

async fn get_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 「卡密不存在或无权限」。
    // 用户正拿着这张卡密（刚在列表里点进来的），却说它不存在。
    let mut card = match db_guard::optional(
        sqlx::query_as::<_, Card>("SELECT * FROM cards WHERE id = $1 AND merchant_id = $2")
            .bind(id)
            .bind(merchant_id)
            .fetch_optional(&state.pool),
        "查询卡密详情",
    )
    .await
    {
        db_guard::QueryOutcome::Found(c) => Some(c),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if let Some(ref mut c) = card {
        match EncryptedFieldsOps::decrypt_card_code(&state.encryptor, &c.code) {
            Ok(plain) => c.code = plain,
            Err(e) => tracing::warn!("解密卡密 {} 失败: {}", c.id, e),
        }
    }

    match card {
        Some(c) => Json(json!({"success": true, "data": c})),
        None => Json(json!({"success": false, "message": "卡密不存在或无权限"})),
    }
}

async fn delete_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = sqlx::query(
        "DELETE FROM cards WHERE id = $1 AND merchant_id = $2 AND status = 'unused'",
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "删除成功"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、已使用或无权限"})),
        Err(e) => db_guard::internal_error("删除卡密", e),
    }
}

async fn disable_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = sqlx::query(
        "UPDATE cards SET status = 'disabled' WHERE id = $1 AND merchant_id = $2 AND admin_disabled = FALSE",
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "卡密已禁用"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在或无权限"})),
        Err(e) => db_guard::internal_error("卡密操作", e),
    }
}

async fn enable_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = sqlx::query(
        "UPDATE cards SET status = 'unused' WHERE id = $1 AND status = 'disabled' AND merchant_id = $2 AND admin_disabled = FALSE",
    )
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "卡密已启用"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在、状态不符或无权限"})),
        Err(e) => db_guard::internal_error("卡密操作", e),
    }
}

/// 单卡延期
async fn extend_card(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<ExtendCardRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if body.days == 0 {
        return Json(json!({"success": false, "message": "days 不能为 0"}));
    }

    let r1 = sqlx::query(
        "UPDATE cards SET duration_days = GREATEST(1, duration_days + $1)
         WHERE id = $2 AND status = 'unused' AND merchant_id = $3"
    )
    .bind(body.days)
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    let r1 = match r1 {
        Ok(r) => r,
        Err(e) => return db_guard::internal_error("卡密延期", e),
    };

    let r2 = sqlx::query(
        "UPDATE cards SET expires_at = GREATEST(NOW(), expires_at + ($1 || ' days')::INTERVAL)
         WHERE id = $2 AND status = 'active' AND expires_at IS NOT NULL AND merchant_id = $3"
    )
    .bind(body.days)
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    let r2 = match r2 {
        Ok(r) => r,
        Err(e) => return db_guard::internal_error("卡密延期", e),
    };

    let affected = r1.rows_affected() + r2.rows_affected();
    if affected > 0 {
        let action = if body.days > 0 { "延期" } else { "缩短" };
        Json(json!({"success": true, "message": format!("已{}{} 天", action, body.days.abs())}))
    } else {
        Json(json!({"success": false, "message": "卡密不存在或无权限"}))
    }
}

/// 编辑备注
async fn update_card_note(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateNoteRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    let result = sqlx::query(
        "UPDATE cards SET note = $1 WHERE id = $2 AND merchant_id = $3"
    )
    .bind(&body.note)
    .bind(id)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true, "message": "备注已更新"})),
        Ok(_) => Json(json!({"success": false, "message": "卡密不存在或无权限"})),
        Err(e) => db_guard::internal_error("卡密操作", e),
    }
}

async fn batch_update_card_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<BatchCardStatusRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if body.ids.is_empty() {
        return Json(json!({"success": false, "message": "ids 不能为空"}));
    }
    if body.ids.len() > 500 {
        return Json(json!({"success": false, "message": "单次批量操作最多 500 张"}));
    }

    let result = match body.action.as_str() {
        "disabled" => {
            sqlx::query(
                "UPDATE cards SET status = 'disabled' WHERE id = ANY($1) AND merchant_id = $2 AND admin_disabled = FALSE",
            )
            .bind(&body.ids)
            .bind(merchant_id)
            .execute(&state.pool)
            .await
        }
        "unused" => {
            sqlx::query(
                "UPDATE cards SET status = 'unused' WHERE id = ANY($1) AND status = 'disabled' AND merchant_id = $2 AND admin_disabled = FALSE",
            )
            .bind(&body.ids)
            .bind(merchant_id)
            .execute(&state.pool)
            .await
        }
        _ => return Json(json!({"success": false, "message": "action 仅支持 disabled / unused"})),
    };

    match result {
        Ok(r) => Json(json!({"success": true, "message": format!("已更新 {} 张卡密", r.rows_affected())})),
        Err(e) => db_guard::internal_error("批量卡密操作", e),
    }
}

async fn batch_extend_cards(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<BatchExtendRequest>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if body.ids.is_empty() {
        return Json(json!({"success": false, "message": "ids 不能为空"}));
    }
    if body.ids.len() > 500 {
        return Json(json!({"success": false, "message": "单次最多操作 500 张"}));
    }
    if body.days == 0 {
        return Json(json!({"success": false, "message": "days 不能为 0"}));
    }

    let r_unused = sqlx::query(
        "UPDATE cards SET duration_days = GREATEST(1, duration_days + $1)
         WHERE id = ANY($2) AND status = 'unused' AND merchant_id = $3"
    )
    .bind(body.days)
    .bind(&body.ids)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    let r_unused = match r_unused {
        Ok(r) => r,
        Err(e) => return db_guard::internal_error("批量卡密延期", e),
    };

    let r_active = sqlx::query(
        "UPDATE cards
         SET expires_at = GREATEST(NOW(), expires_at + ($1 || ' days')::INTERVAL)
         WHERE id = ANY($2) AND status = 'active' AND expires_at IS NOT NULL AND merchant_id = $3"
    )
    .bind(body.days)
    .bind(&body.ids)
    .bind(merchant_id)
    .execute(&state.pool)
    .await;

    let r_active = match r_active {
        Ok(r) => r,
        Err(e) => return db_guard::internal_error("批量卡密延期", e),
    };

    let total = r_unused.rows_affected() + r_active.rows_affected();
    let action = if body.days > 0 { "延期" } else { "缩短" };
    Json(json!({
        "success": true,
        "message": format!("已{}{}张卡密（未激活 {}，已激活 {}）", action, total, r_unused.rows_affected(), r_active.rows_affected()),
        "updated": total,
    }))
}

async fn card_group_stats(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let merchant_id = match merchant_id_from_claims(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    // ⚠️ 曾用 .unwrap_or_default()：分组统计查询失败时返回空数组，
    // 商户看到「卡密分组」页全空，会误以为卡密被删光（实际是统计查询挂了），
    // 进而去找根本不存在的「误删」问题。
    let rows: Vec<CardGroupStat> = match sqlx::query_as(
        r#"SELECT
            duration_days,
            max_devices,
            COUNT(*)                                       AS total,
            COUNT(*) FILTER (WHERE status = 'unused')      AS unused,
            COUNT(*) FILTER (WHERE status = 'active')      AS active,
            COUNT(*) FILTER (WHERE status = 'expired')     AS expired,
            COUNT(*) FILTER (WHERE status = 'disabled')    AS disabled
         FROM cards
         WHERE merchant_id = $1
         GROUP BY duration_days, max_devices
         ORDER BY duration_days, max_devices"#,
    )
    .bind(merchant_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("查询卡密分组统计失败: err={}", e);
            return db_guard::server_busy();
        }
    };

    Json(json!({"success": true, "data": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 基本情形：配额内放行、超配额拒绝
    #[test]
    fn quota_allows_within_limit_and_denies_beyond() {
        // used=0, need=3, total=3 → 恰好用满，应放行
        assert_eq!(judge_agent_quota(3, 0, 3), QuotaVerdict::Allow);
        // 再多一张就超了
        assert!(matches!(judge_agent_quota(3, 0, 4), QuotaVerdict::Deny(_)));
        // 已用 2，再来 1 张恰好用满
        assert_eq!(judge_agent_quota(3, 2, 1), QuotaVerdict::Allow);
        // 已用 2，再来 2 张超 1 张
        assert!(matches!(judge_agent_quota(3, 2, 2), QuotaVerdict::Deny(_)));
    }

    /// 这是原 bug 的核心场景：代理**从不激活**，只反复生成。
    ///
    /// 原实现里 `quota_used` 只在激活时 +1，所以这里的 used 永远是 0，
    /// 无论生成多少张都放行 —— 配额形同虚设。
    /// 新口径下 used 必须来自 `COUNT(cards)`，累计生成会撞上总额度。
    #[test]
    fn quota_is_consumed_by_generation_not_by_activation() {
        let total = 3;
        let mut generated = 0i64;
        let mut allowed = 0;
        // 连发 10 次 count=1，全程不激活
        for _ in 0..10 {
            if judge_agent_quota(total, generated, 1) == QuotaVerdict::Allow {
                generated += 1; // 生成成功，已用量增加
                allowed += 1;
            }
        }
        assert_eq!(allowed, 3, "配额 3 就只该允许生成 3 张，与实际 {allowed} 不符");
        assert_eq!(generated, 3);
    }

    /// `quota_total < 0` = 不限制（与套餐 -1 = unlimited 的约定一致）
    #[test]
    fn negative_quota_means_unlimited() {
        assert_eq!(judge_agent_quota(-1, 999_999, 1000), QuotaVerdict::Allow);
        assert_eq!(judge_agent_quota(-1, 0, 0), QuotaVerdict::Allow);
    }

    /// `quota_total == 0` = 一张都不许（不是"未设置"）
    #[test]
    fn zero_quota_denies_everything() {
        assert!(matches!(judge_agent_quota(0, 0, 1), QuotaVerdict::Deny(_)));
        assert_eq!(judge_agent_quota(0, 0, 0), QuotaVerdict::Allow, "生成 0 张无需拒绝");
    }

    /// 已用量已经超出配额（比如上级后来把配额调小了）时：
    /// 剩余量不能算成负数，否则提示文案会出现「剩余 -2」这种鬼话。
    #[test]
    fn remaining_is_clamped_to_zero_when_already_over_quota() {
        match judge_agent_quota(3, 5, 1) {
            QuotaVerdict::Deny(msg) => {
                assert!(msg.contains("剩余 0"), "剩余量应为 0，实际消息: {msg}");
                assert!(!msg.contains("剩余 -"), "出现了负数剩余量: {msg}");
                assert!(msg.contains("已用 5"), "应显示真实已用量，实际: {msg}");
            }
            other => panic!("应拒绝，实际 {other:?}"),
        }
    }

    /// 拒绝消息必须把「需要多少 / 已用多少 / 剩余多少」都写清楚 ——
    /// 只写「配额不足」用户无法判断该升级多少。
    #[test]
    fn deny_message_carries_actionable_numbers() {
        match judge_agent_quota(100, 90, 20) {
            QuotaVerdict::Deny(msg) => {
                assert!(msg.contains("100"), "缺总额度: {msg}");
                assert!(msg.contains("90"), "缺已用量: {msg}");
                assert!(msg.contains("10"), "缺剩余量: {msg}");
                assert!(msg.contains("20"), "缺本次需要量: {msg}");
            }
            other => panic!("应拒绝，实际 {other:?}"),
        }
    }

    /// i64 大数不应溢出（used/need 来自 COUNT(*) 与 u32，理论上不会，
    /// 但配额判定是"最后一道闸"，用溢出绕过它是最典型的攻击面）
    #[test]
    fn huge_numbers_do_not_overflow_into_allow() {
        // used + need 若用 i32 相加会溢出成负数 → 被判「在配额内」
        let verdict = judge_agent_quota(100, i64::from(i32::MAX), i64::from(i32::MAX));
        assert!(matches!(verdict, QuotaVerdict::Deny(_)), "大数相加必须仍然拒绝");
    }

    // ── CardStatusFilter 归一化 ──

    /// 三个合法值都能正确归一化；非法值返回 None（由调用方视为不过滤）
    #[test]
    fn status_filter_parse_known_values() {
        assert_eq!(CardStatusFilter::parse("unused"), Some(CardStatusFilter::Unused));
        assert_eq!(CardStatusFilter::parse("active"), Some(CardStatusFilter::Active));
        assert_eq!(CardStatusFilter::parse("expired"), Some(CardStatusFilter::Expired));
        assert_eq!(CardStatusFilter::parse("bogus"), None);
        assert_eq!(CardStatusFilter::parse(""), None);
    }

    /// 「已过期」必须实时按 expires_at 判断，而不是 `status='expired'`（懒标记）。
    /// 这个测试钉住的是语义：片段里必须出现 `expires_at <= NOW()`，
    /// 否则一张「到期但没再激活、status 仍是 active」的卡会从过期过滤里漏掉。
    #[test]
    fn expired_filter_uses_expires_at_not_status() {
        let fragment = CardStatusFilter::Expired.sql_fragment();
        assert!(
            fragment.contains("expires_at <= NOW()"),
            "过期过滤必须实时判断 expires_at，实际片段: {fragment}"
        );
        // 不能出现 `status = 'expired'`（那会漏掉懒标记场景）
        assert!(
            !fragment.contains("status = 'expired'"),
            "过期过滤不应依赖懒标记的 status，实际片段: {fragment}"
        );
    }

    /// 「已激活」必须排除已到期的卡（expires_at 已过但 status 仍是 active 的）
    #[test]
    fn active_filter_excludes_expired() {
        let fragment = CardStatusFilter::Active.sql_fragment();
        assert!(fragment.contains("status = 'active'"));
        assert!(
            fragment.contains("expires_at IS NULL OR expires_at > NOW()"),
            "已激活过滤必须排除到期卡，实际片段: {fragment}"
        );
    }
}
