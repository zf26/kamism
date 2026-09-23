//! 数据库查询的静默失败防护
//!
//! ────────────────────────────────────────────────────────────────────────────
//! 这个模块存在的唯一理由：`sqlx` 的 `Result` 很容易被 `.unwrap_or(None)`
//! 压成「查不到」，而「查不到」和「查不了」是**完全不同的两件事**。
//! ────────────────────────────────────────────────────────────────────────────
//!
//! 典型事故（本仓库真实存在过）：
//!
//! ```ignore
//! let merchant: Option<(Uuid,)> =
//!     sqlx::query_as("SELECT id FROM merchants WHERE api_key_hash = $1 AND status = 'active'")
//!         .bind(&api_key_hash)
//!         .fetch_optional(&state.pool)
//!         .await
//!         .unwrap_or(None);          // ← 数据库抖动被伪装成「查不到」
//!
//! let merchant_id = match merchant {
//!     Some((id,)) => id,
//!     None => return Json(json!({"success": false, "message": "无效的 API Key"})),
//! };
//! ```
//!
//! 数据库一抖，**所有商户同时收到「无效的 API Key」** ——
//! 而且 `tracing` 里一个字都没有。排查的人会去查密钥，而真正的问题在数据库。
//!
//! 更危险的是**方向相反**的情况（黑名单查询）：
//!
//! ```ignore
//! let blocked = sqlx::query_as("SELECT 1 FROM ip_blacklist WHERE ...")
//!     .fetch_optional(&state.pool).await
//!     .unwrap_or(None);          // ← 查询失败 → 「没被拉黑」→ 风控放行
//! if blocked.is_some() { return 拒绝; }
//! ```
//!
//! 这里的默认值是**放宽**了限制 —— 故障变成了「服务照常」，没有任何人会察觉到风控已经失效。
//!
//! ── 用法 ────────────────────────────────────────────────────────────────────
//!
//! ```ignore
//! // ① 查不到就是业务结论（类 A）
//! let card = match fetch_optional!(sqlx::query_as("SELECT ...").bind(x).fetch_optional(pool), "查询卡密") {
//!     Ok(Some(c)) => c,
//!     Ok(None) => return Json(json!({"success": false, "message": "卡密不存在"})),
//!     Err(resp) => return Json(resp),      // 故障 → 统一文案
//! };
//! ```
//!
//! 不提供宏，改用下面的三个函数 —— 宏会藏住控制流，
//! 而这个模块的目的恰恰是**让控制流显式**。

use axum::Json;
use serde_json::{json, Value};

/// 「服务器内部故障」的统一响应体。
///
/// 为什么统一：客户端不需要知道是数据库挂了还是 Redis 挂了，
/// 而**具体原因必须进日志、不能进响应体** —— 把 `sqlx::Error` 的原文返回给
/// 客户端会泄漏表名、列名、约束名，甚至 SQL 片段。
/// （`utils::error::AppError::Database` 曾经也是把原文拼进 message 的 ——
///  第十一批已把它一并修安全：原文只进日志，响应体回统一文案。
///  那个类型至今**没有调用点**，修它纯粹是为了拆掉雷。）
pub fn server_busy() -> Json<Value> {
    Json(json!({"success": false, "message": "服务器繁忙，请稍后重试"}))
}

/// 「服务端操作失败」的统一响应 —— **带日志上下文**，替代全仓库那几十处
/// `format!("创建失败: {}", e)`。
///
/// `what` 是给人看的中文描述（如「删除卡密」），会拼进日志；
/// 响应体里**只回统一文案**，不带任何错误原文。
///
/// ── 为什么必须有这个函数 ────────────────────────────────────────────────
///
/// `sqlx::Error` 的 `Display` 是**数据库原文**，不是抽象描述。实测两例：
///
/// ```text
/// duplicate key value violates unique constraint "cards_code_hash_key"
/// 字段 "plan" 不存在
/// ```
///
/// 前者泄漏**表名 + 列名 + 约束名**，后者泄漏 **schema 结构**。
/// 而本项目有几处这样的响应是**未登录就能打**的：
/// `/auth/register`、`/auth/reset-password`、`/v1/activate`。
/// 攻击者不需要先制造故障 —— 只要构造能撞上约束的输入，
/// 就能靠这条错误信息把表结构一条条问出来。
///
/// 上面 `server_busy()` 的注释早就写了这条规矩，只是当时只有一个**无参**版本，
/// 于是几十个调用点各自 `format!("…: {}", e)` 去了 —— 规矩没落地成可用的形状。
///
/// ── 用法 ──────────────────────────────────────────────────────────────
///
/// ```ignore
/// match sqlx::query("DELETE FROM cards WHERE id = $1").execute(&state.pool).await {
///     Ok(r) if r.rows_affected() > 0 => Json(json!({"success": true})),
///     Ok(_) => Json(json!({"success": false, "message": "卡密不存在"})),  // 业务结论，照常回
///     Err(e) => db_guard::internal_error("删除卡密", e),                   // 故障，只进日志
/// }
/// ```
///
/// ⚠️ `Ok(_)`（影响 0 行）与 `Err(_)` 是**两件不同的事**：前者是「不存在」，
/// 该如实告诉用户；后者是「查不了」，只能回统一文案。
/// 把两者合并回一个分支，就是本模块开头说的那次静默合并。
pub fn internal_error(what: &str, e: impl std::fmt::Display) -> Json<Value> {
    tracing::error!("{}失败: {}", what, e);
    server_busy()
}

/// 判断是不是**唯一约束冲突**（该告诉用户「已存在」，而不是「服务器繁忙」）。
///
/// ── 为什么必须有这个函数 ────────────────────────────────────────────────
///
/// 原先 `subscription_plan.rs` 写的是：
///
/// ```ignore
/// if e.to_string().contains("duplicate key") { … }
/// ```
///
/// 那是拿**数据库自己那句话**当控制流，而那句话是随 `lc_messages` 变的。
/// 本机 PG（中文）实测报的是：
///
/// ```text
/// 错误:  重复键违反唯一约束"uq_subscription_plans_plan_days"
/// DETAIL:  键值"(plan, days)=(pro, 30)" 已经存在
/// ```
///
/// `contains("duplicate key")` **永远匹配不上** → 管理员建重复套餐，
/// 看到的是「服务器繁忙，请稍后重试」—— 一个**输入错误被报成了服务器故障**。
///
/// ── 为什么用 `is_unique_violation()` 而不是硬编码 "23505" ────────────────
///
/// `sqlx-postgres` 已经把 PG 的 SQLSTATE 映射成 `ErrorKind`
/// （`error_codes::UNIQUE_VIOLATION = "23505"` → `ErrorKind::UniqueViolation`），
/// 而 `DatabaseError::is_unique_violation()` 就是查这个 kind。
/// SQLSTATE 是**协议层**字段，不受语言 / PG 版本 / 约束名影响；
/// 交给 sqlx 映射比自己写字符串少一个可能过时的常量。
///
/// ⚠️ 注意它**不看 message**：即使 message 里恰好出现 "duplicate key"，
/// 只要 kind 不是 UniqueViolation 就返回 false（单测钉住了这一点）。
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}

/// `fetch_optional` 的结果，把 `sqlx::Error` 收敛成一个可判定的枚举。
///
/// 用这个枚举代替 `unwrap_or(None)`：
/// ```ignore
/// let r = fetch_opt(sqlx::query_as(...).bind(x).fetch_optional(pool).await, "查询商户");
/// let merchant = match r {
///     QueryOutcome::Found(m) => m,
///     QueryOutcome::NotFound => return Json(json!({"success": false, "message": "无效的 API Key"})),
///     QueryOutcome::Failed => return server_busy(),
/// };
/// ```
///
/// 关键点：`NotFound` 和 `Failed` 是**不同的变体**，编译器不允许把它们合并 ——
/// 而 `unwrap_or(None)` 恰好就是那次静默的合并。
pub enum QueryOutcome<T> {
    /// 查到了
    Found(T),
    /// 查通了，但没有匹配的行（**这是正常的业务结果**）
    NotFound,
    /// 查询本身失败（数据库不可用 / SQL 错误 / 连接池耗尽…）
    Failed,
}

/// 执行一个 `fetch_optional` 并归类结果，顺带记录错误日志。
///
/// `what` 是给人看的中文描述（例如「查询商户 API Key」），
/// 会拼进日志里。不要传敏感值 —— 日志会被收集和展示。
pub async fn optional<T, F>(fut: F, what: &str) -> QueryOutcome<T>
where
    F: std::future::Future<Output = Result<Option<T>, sqlx::Error>>,
{
    match fut.await {
        Ok(Some(v)) => QueryOutcome::Found(v),
        Ok(None) => QueryOutcome::NotFound,
        Err(e) => {
            tracing::error!("{}失败（数据库错误，不是「没有数据」）: {}", what, e);
            QueryOutcome::Failed
        }
    }
}

/// 同上，但对象是 `fetch_one`（聚合查询，一定有一行）。
///
/// 聚合查询（`SELECT COUNT(*)` / `SUM(...)`）在**语法上**永远返回一行，
/// 所以这里没有 `NotFound`。但「查询失败 → 用默认值 0 继续」是同类错误：
/// 例如设备异常检测会因此**静默放行**（见本文件头部说明）。
pub enum ScalarOutcome<T> {
    Found(T),
    Failed,
}

/// 执行 `fetch_one` 并归类结果。
pub async fn scalar<T, F>(fut: F, what: &str) -> ScalarOutcome<T>
where
    F: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    match fut.await {
        Ok(v) => ScalarOutcome::Found(v),
        Err(e) => {
            tracing::error!("{}失败（数据库错误）: {}", what, e);
            ScalarOutcome::Failed
        }
    }
}

/// 把 `QueryOutcome` 直接转成 `Option<T>`，**但会记日志**。
///
/// ⚠️ 只在「失败和不存在可以合并且都走同一条降级路径」时使用（类 C）。
/// 不要用它来偷懒 —— 它的存在是为了让降级**留下痕迹**，
/// 而不是为了让代码短一点。
pub async fn optional_lenient<T, F>(fut: F, what: &str) -> Option<T>
where
    F: std::future::Future<Output = Result<Option<T>, sqlx::Error>>,
{
    match fut.await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{}失败，按「不存在」降级处理（该降级是有意的）: {}", what, e);
            None
        }
    }
}

/// 同 `optional_lenient`，但对象是 `fetch_all`（返回 `Vec<T>`）。
///
/// 单独一个函数而不是复用 `optional_lenient`：`fetch_all` 的返回类型是
/// `Result<Vec<T>, _>` 而不是 `Result<Option<T>, _>`，两者在类型上无法统一。
/// 而「空列表」正是**最容易伪装成正常结果**的降级值 ——
/// 页面上显示「暂无数据」时，没人分得清是真的没有还是查不到。
/// 所以这里的 `what` 参数要写清楚降级后界面上会显示成什么。
pub async fn lenient_all<T, F>(fut: F, what: &str) -> Vec<T>
where
    F: std::future::Future<Output = Result<Vec<T>, sqlx::Error>>,
{
    match fut.await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{}失败，按「空列表」降级处理（该降级是有意的）: {}", what, e);
            Vec::new()
        }
    }
}

// 📌 曾经这里有个 `classify_exists` / `ExistsOutcome`，用于把
// `QueryOutcome<(Uuid,)>` 换名成 Yes/No/Failed。写调用点时才发现它是多余的：
// 调用点照样要 match 三个变体，只是把 `Found` 念成 `Yes` —— 换来的是
// 多一层间接、少一次编译器的穷尽性提示。删掉了。
// 需要存在性检查时直接用 `optional`，三个变体已经足够表意。

#[cfg(test)]
mod tests {
    use super::*;

    /// 三条分支必须互相区分开 —— 这是本模块的全部价值。
    ///
    /// 如果 `NotFound` 和 `Failed` 被合并（像 `unwrap_or(None)` 那样），
    /// 这个测试就无从写起 —— 这正是它存在的意义。
    #[tokio::test]
    async fn outcome_distinguishes_not_found_from_failure() {
        // 模拟 fetch_optional 的三种返回
        let found: Result<Option<i32>, sqlx::Error> = Ok(Some(1));
        let none: Result<Option<i32>, sqlx::Error> = Ok(None);

        match optional(async { found }, "测试").await {
            QueryOutcome::Found(v) => assert_eq!(v, 1),
            _ => panic!("Ok(Some) 必须归为 Found"),
        }

        match optional(async { none }, "测试").await {
            QueryOutcome::NotFound => {}
            _ => panic!("Ok(None) 必须归为 NotFound（这是正常的空集语义，不是故障）"),
        }

        // sqlx::Error 造一个真实的：PoolClosed 最简单
        let err: Result<Option<i32>, sqlx::Error> = Err(sqlx::Error::PoolClosed);
        match optional(async { err }, "测试").await {
            QueryOutcome::Failed => {}
            _ => panic!("Err 必须归为 Failed —— 这正是 unwrap_or(None) 抹掉的那条分支"),
        }
    }

    #[tokio::test]
    async fn scalar_distinguishes_value_from_failure() {
        let ok: Result<i32, sqlx::Error> = Ok(0);
        match scalar(async { ok }, "测试").await {
            ScalarOutcome::Found(v) => assert_eq!(v, 0),
            _ => panic!("Ok 必须归为 Found"),
        }

        let err: Result<i32, sqlx::Error> = Err(sqlx::Error::PoolClosed);
        match scalar(async { err }, "测试").await {
            ScalarOutcome::Failed => {}
            _ => panic!("Err 必须归为 Failed"),
        }
    }

    /// `optional_lenient` 仍然区分二者，只是把失败记成 warn 后返回 None。
    /// 关键是它**不会静默** —— 日志里有记录。
    #[tokio::test]
    async fn lenient_still_logs_the_failure() {
        let err: Result<Option<i32>, sqlx::Error> = Err(sqlx::Error::PoolClosed);
        assert!(optional_lenient(async { err }, "测试").await.is_none());

        let none: Result<Option<i32>, sqlx::Error> = Ok(None);
        assert!(optional_lenient(async { none }, "测试").await.is_none());
        // 两者都返回 None —— 所以这个函数**只**能用在允许合并的场景。
        // 需要区分时用 `optional`。这个测试把这个限制写明了。
    }

    /// `lenient_all` 的降级值是**空列表**，所以测试要同时确认两件事：
    /// `Ok(vec![])` 和 `Err` 都返回空 —— 这正是它危险的地方，
    /// 也解释了为什么它必须记 warn（否则两种输入在调用方看来一模一样）。
    #[tokio::test]
    async fn lenient_all_degrades_to_empty_but_is_intentionally_so() {
        let err: Result<Vec<i32>, sqlx::Error> = Err(sqlx::Error::PoolClosed);
        assert!(lenient_all(async { err }, "测试").await.is_empty());

        let empty: Result<Vec<i32>, sqlx::Error> = Ok(vec![]);
        assert!(lenient_all(async { empty }, "测试").await.is_empty());

        // 非空时要原样返回（确认它不是无脑返回空）
        let some: Result<Vec<i32>, sqlx::Error> = Ok(vec![1, 2, 3]);
        assert_eq!(lenient_all(async { some }, "测试").await, vec![1, 2, 3]);
    }

    /// `internal_error` 的响应体**只**能是统一文案 —— 不带任何错误原文。
    ///
    /// 这是本批（第十一批）替换掉全仓库几十处 `format!("XX失败: {}", e)`
    /// 之后唯一能自动盯住它的地方：那些调用点分散在十几个文件里，
    /// 端到端只能按路径一条条测，而这条对**所有**调用点都成立。
    #[tokio::test]
    async fn internal_error_never_leaks_the_error_text() {
        use axum::response::IntoResponse;
        let resp = internal_error("测试操作", sqlx::Error::RowNotFound).into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let s = String::from_utf8_lossy(&bytes).to_string();
        assert!(
            !s.contains("rows"),
            "响应体泄漏了 sqlx 原文: {}",
            s
        );
        assert!(s.contains("服务器繁忙"), "响应体应是统一文案: {}", s);
    }

    // ── is_unique_violation ───────────────────────────────────────────────

    /// 假的 `sqlx::error::DatabaseError`。
    ///
    /// 为什么要自己实现：`sqlx::Error::Database` 装的是 `Box<dyn DatabaseError>`，
    /// 而 PG 的真实实现只有真连上库、真撞上约束才拿得到 —— 单测里没有库。
    /// 这个 trait 有五个必需方法（`message` / `kind` / `as_error` /
    /// `as_error_mut` / `into_error`），其余都有默认实现 ——
    /// 其中就包括我们要测的 `is_unique_violation()`（默认实现是
    /// `matches!(self.kind(), ErrorKind::UniqueViolation)`）。
    ///
    /// `ErrorKind` 既不是 `Copy` 也不是 `Clone` 且带 `#[non_exhaustive]`，
    /// 所以这里用布尔开关来决定 kind，而不是存一个 `ErrorKind` 再返回 ——
    /// 那样得写一个带通配臂的 match，纯属噪音。
    #[derive(Debug)]
    struct FakeDbError {
        message: String,
        unique: bool,
    }

    impl std::fmt::Display for FakeDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.message)
        }
    }
    impl std::error::Error for FakeDbError {}

    impl sqlx::error::DatabaseError for FakeDbError {
        fn message(&self) -> &str {
            &self.message
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            if self.unique {
                sqlx::error::ErrorKind::UniqueViolation
            } else {
                sqlx::error::ErrorKind::Other
            }
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
    }

    fn db_err(message: &str, unique: bool) -> sqlx::Error {
        sqlx::Error::Database(Box::new(FakeDbError {
            message: message.to_string(),
            unique,
        }))
    }

    #[test]
    fn unique_violation_is_recognised() {
        assert!(is_unique_violation(&db_err("重复键违反唯一约束\"uq_x\"", true)));
    }

    #[test]
    fn non_database_errors_are_not_unique_violations() {
        assert!(!is_unique_violation(&sqlx::Error::PoolClosed));
        assert!(!is_unique_violation(&sqlx::Error::RowNotFound));
        // 其它 kind 的 Database 错误也必须是 false —— 否则会把外键/非空冲突
        // 也报成「已存在」，那是另一种「说错话」。
        assert!(!is_unique_violation(&db_err("外键冲突", false)));
    }

    /// 🎯 这条是本函数的**核心**：判断依据是 `kind`，**不是** message 文本。
    ///
    /// 两句 message 分别对应「原实现会误判」和「原实现会漏判」两个方向，
    /// 而期望结果与 message 完全无关。
    #[test]
    fn decision_looks_at_kind_not_at_message_text() {
        // 方向一：message 里明明白白写着 "duplicate key"
        // ——正是原实现 `contains("duplicate key")` 会命中的形状——
        // 但 kind 不是唯一冲突，所以必须判 false。
        let would_misjudge = db_err(
            "duplicate key value violates unique constraint \"cards_code_hash_key\"",
            false,
        );
        assert!(
            !is_unique_violation(&would_misjudge),
            "不能因为 message 里出现 duplicate key 就判成唯一冲突 —— 那正是原实现的错"
        );

        // 方向二：message 是中文（本机 PG 的实际输出），原实现**永远匹配不上**。
        // kind 对，就必须认出来。
        let would_miss = db_err(
            "重复键违反唯一约束\"uq_subscription_plans_plan_days\"",
            true,
        );
        assert!(
            is_unique_violation(&would_miss),
            "中文 message 下必须仍然认出唯一冲突 —— lc_messages 不该影响判断"
        );
    }
}
