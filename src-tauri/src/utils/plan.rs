//! 套餐有效期的**唯一**写入口径。
//!
//! ── 为什么必须只有一处 ──────────────────────────────────────────────────
//!
//! 改造前，同一件事（"给商户 N 天专业版"）在三个地方有三种算法：
//!
//! | 入口 | 旧 SQL | 后果 |
//! |---|---|---|
//! | 商户自助续费（支付回调） | `COALESCE(plan_expires_at, NOW()) + N` | 已过期商户付钱后，到期日仍在**过去** → 下一轮扫描立刻降级，**钱白付** |
//! | 管理员「升专业版」 | `NOW() + N` | **覆盖**：原本还剩 100 天，选 30 天反而缩短成 30 天 |
//! | 管理员「续期」 | `GREATEST(plan_expires_at, NOW()) + N` | 正确，但只有这一处对 |
//!
//! 三种算法里两种会在特定场景损害用户或平台，而且**都不是崩溃**：
//! 接口返回成功、界面显示成功、库里的值也真的变了，只是变错了。
//! 这类问题最难解释，也最容易演变成投诉和退款。
//!
//! ── 现在的口径（唯一）────────────────────────────────────────────────────
//!
//! * 原本是专业版且在有效期内 → **在剩余时间上叠加**（买多少天就多多少天）
//! * 原本是专业版但已过期   → **从现在起算**（过期那段时间不补，也不倒扣）
//! * 原本是免费版           → **从现在起算**（不继承任何历史到期时间）
//! * 已经是**永久**专业版   → **不动**，返回 [`ExtendOutcome::Permanent`] 由调用方提示
//!
//! 永久版这一条特别重要：`GREATEST(NULL, NOW())` 在 PostgreSQL 里返回 `NULL`
//! （`GREATEST` 遇 NULL 直接取 NULL），所以天真写法会把"永久"算成 `NULL + N 天 = NULL`，
//! `rows_affected` 却是 1 —— 界面提示"续期成功"，实际什么都没发生。
//! 更糟的写法是 `COALESCE`，它会把永久版**降级**成 N 天。
//!
//! ── 为什么不在这里一起处理「设为永久 / 降为免费」────────────────────────
//!
//! 那两个动作是**赋值**语义（把到期日设成 NULL），不是"延长"语义，
//! 与管理员的显式意图一一对应，混进 `extend_plan` 只会让这个函数变成
//! 「什么都能干」的开关。它们留在 admin.rs 的对应分支里，注释已标明是赋值。

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// 续期结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendOutcome {
    /// 已延长，附新的到期时间
    Extended { expires_at: DateTime<Utc> },
    /// 已是**永久**专业版：本次**没有**改动任何数据（见模块注释）
    Permanent,
    /// 商户不存在（或并发被删）
    NotFound,
}

/// 在商户当前到期时间上叠加 `days` 天，并把套餐置为专业版。
///
/// `days` 由调用方校验（本项目统一为 1 ~ 3650）。
///
/// ⚠️ `conn` 必须是调用方自己的连接 / 事务：
/// * 支付回调里传 `&mut *tx` —— 续期必须和「订单置为已支付」在**同一个事务**里，
///   否则会出现"钱记了、时长没加"或反之；
/// * 管理端传 `&mut *pool.acquire()`。
///
/// 之所以不写成泛型 `E: Executor`：本函数在**未命中**时还要再查一次
/// "到底是永久版还是不存在"，而 `Executor` 是一次性消耗的，
/// 泛型版本没法把同一个 executor 用两次（要么多一层 trait，要么把诊断逻辑
/// 推给每个调用方重复写一遍 —— 那正是这个模块要消灭的东西）。
pub async fn extend_plan(
    conn: &mut sqlx::PgConnection,
    merchant_id: Uuid,
    days: i64,
) -> Result<ExtendOutcome, sqlx::Error> {
    // 一条语句完成「读旧值 + 判永久 + 写新值」：
    //   * `before` 这个 CTE 拿到**改动前**的 plan / 到期时间；
    //   * `NOT (b.plan = 'pro' AND b.plan_expires_at IS NULL)` 把永久版挡在 UPDATE 之外，
    //     所以"永久版被续期"不会变成一次假成功（没命中 → 走下面的诊断）；
    //   * SET 表达式里引用的是 `m.plan_expires_at`（目标行最新值），
    //     所以并发续期不会互相覆盖成同一个结果。
    let row: Option<(DateTime<Utc>,)> = sqlx::query_as(
        r#"
        WITH before AS (
            SELECT id, plan, plan_expires_at
              FROM merchants
             WHERE id = $1
        )
        UPDATE merchants m
           SET plan = 'pro',
               plan_expires_at = CASE
                   -- 专业版：剩余时间上叠加；已过期则从现在起算（GREATEST 兜住）
                   WHEN b.plan = 'pro'
                       THEN GREATEST(COALESCE(m.plan_expires_at, NOW()), NOW())
                   -- 免费版：一律从现在起算，不继承任何历史到期时间
                   ELSE NOW()
               END + ($2 || ' days')::INTERVAL,
               updated_at = NOW()
          FROM before b
         WHERE m.id = b.id
           AND NOT (b.plan = 'pro' AND b.plan_expires_at IS NULL)
        RETURNING m.plan_expires_at
        "#,
    )
    .bind(merchant_id)
    .bind(days.to_string())
    .fetch_optional(&mut *conn)
    .await?;

    if let Some((expires_at,)) = row {
        return Ok(ExtendOutcome::Extended { expires_at });
    }

    // 没命中：要么不存在，要么是永久专业版（被 WHERE 挡住）。
    // 这条诊断查询只在失败路径上跑一次，正常续期永远多不出这一次往返。
    let state: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT plan, plan_expires_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(&mut *conn)
    .await?;

    Ok(match state {
        // 永久专业版：`plan = 'pro'` 且到期时间为 NULL
        Some((ref plan, None)) if plan == "pro" => ExtendOutcome::Permanent,
        // 其余情况（不存在 / 刚好被并发删除）按不存在处理
        _ => ExtendOutcome::NotFound,
    })
}
