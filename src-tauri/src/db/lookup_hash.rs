//! 查找哈希的**算法版本标记**与**一次性回填**。
//!
//! ── 背景 ─────────────────────────────────────────────────────────────────
//!
//! `encrypted_fields::generate_hash` 从裸 SHA-256（v1）换成了
//! `HMAC-SHA256(pepper, value)`（v2）。这些哈希列是用来做
//! `WHERE xxx_hash = $1` 精确匹配的，所以**算法一换，存量行立刻查不到** ——
//! 表现为「用户拿着正确的卡密被告知不存在」「登录说邮箱没注册过」，
//! 而库连接正常、接口返回 200、日志干净。这是本项目最忌讳的那种失效。
//!
//! 所以换算法必须配一次性回填。本模块就是那一步。
//!
//! ── 为什么需要一个「版本标记」而不是直接回填 ──────────────────────────────
//!
//! 「忘了跑回填」和「回填跑了一半」都必须**在启动时大声报错**，
//! 而不是等用户报「卡密用不了」。标记写在 `encryption_keys` 表里 ——
//! 这张表的语义就是「库里数据用的是什么密钥材料」，而且在本仓库里
//! **零引用**（建了表一直没接线），正好是现成的空位，不需要新的 schema 变更。
//!
//! ── 哪些列能回填、哪些不能 ───────────────────────────────────────────────
//!
//! 回填的前提是**能拿回明文**。四列可以（明文在 `*_encrypted` 里，AES-256-GCM）：
//!
//! | 表 | 密文列 | 哈希列 |
//! |---|---|---|
//! | `merchants` | `api_key_encrypted` | `api_key_hash` |
//! | `merchants` | `email_encrypted` | `email_hash` |
//! | `cards` | `code_encrypted` | `code_hash` |
//! | `activations` | `device_id_encrypted` | `device_id_hash` |
//!
//! **`device_blacklist.device_id_hash` 回填不了** —— 那张表只有
//! `device_id_hash` 和 `device_hint`（掩码展示用），**没有 `*_encrypted` 列**，
//! 明文从来没落过库。处置是运行时同时匹配新旧两种哈希
//! （唯一一处 `generate_hash_legacy_sha256` 调用点，见
//! `routes/public_api.rs` 的黑名单检查），而不是「不管了」——
//! 不管的后果是被封设备**静默解封**。
//!
//! `merchants.github_id / google_id / microsoft_id` 也是哈希（同样是
//! `generate_hash` 的输出），但它们**全仓库只写不读**（OAuth 登录按
//! `email_hash` 找行，见 `routes/oauth.rs`），所以换算法不影响任何查询，
//! 无需回填 —— 但这意味着它们里面存的仍是旧算法的值，记录在案。

use crate::db::encrypted_fields::EncryptedFieldsOps;
use crate::db::DbPool;
use crate::utils::kms::Encryptor;
use anyhow::{Context, Result};
use uuid::Uuid;

/// 版本标记在 `encryption_keys` 里的 `key_id`
pub const MARKER_KEY_ID: &str = "lookup_hash";

/// v1 = 裸 SHA-256（历史值，仅 `device_blacklist` 存量行还在用）
/// v2 = HMAC-SHA256(pepper, value)，即 `generate_hash` 的当前实现
pub const V1_BARE_SHA256: i32 = 1;
pub const V2_HMAC_SHA256: i32 = 2;

/// 可回填的 (表, 密文列, 哈希列) 组合。列名全是本文件的常量，不含用户输入，
/// 所以下面的 `format!` 拼 SQL 不构成注入面。
const REHASHABLE: &[(&str, &str, &str)] = &[
    ("merchants", "api_key_encrypted", "api_key_hash"),
    ("merchants", "email_encrypted", "email_hash"),
    ("cards", "code_encrypted", "code_hash"),
    ("activations", "device_id_encrypted", "device_id_hash"),
];

/// 存有查找哈希的表（用于「库里有没有数据」的判断）
pub struct RowCounts {
    pub merchants: i64,
    pub cards: i64,
    pub activations: i64,
    pub device_blacklist: i64,
}

impl RowCounts {
    pub fn total(&self) -> i64 {
        self.merchants + self.cards + self.activations + self.device_blacklist
    }

    pub fn describe(&self) -> String {
        format!(
            "merchants={} cards={} activations={} device_blacklist={}",
            self.merchants, self.cards, self.activations, self.device_blacklist
        )
    }
}

/// 各表当前行数。查询失败**不能**当成 0 —— 那会让「库连不上」看起来
/// 像「全新部署」，于是启动守卫放行、标记被写成 v2，回填永远没人跑。
pub async fn row_counts(pool: &DbPool) -> Result<RowCounts> {
    let (merchants,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM merchants")
        .fetch_one(pool)
        .await
        .context("统计 merchants 行数失败（无法判断是否需要回填）")?;
    let (cards,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cards")
        .fetch_one(pool)
        .await
        .context("统计 cards 行数失败（无法判断是否需要回填）")?;
    let (activations,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM activations")
        .fetch_one(pool)
        .await
        .context("统计 activations 行数失败（无法判断是否需要回填）")?;
    let (device_blacklist,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_blacklist")
        .fetch_one(pool)
        .await
        .context("统计 device_blacklist 行数失败（无法判断是否需要回填）")?;

    Ok(RowCounts {
        merchants,
        cards,
        activations,
        device_blacklist,
    })
}

/// 读回填标记。`Ok(None)` = 从未标记过（全新库，或还没跑过回填）。
pub async fn read_marker(pool: &DbPool) -> Result<Option<i32>> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT key_version FROM encryption_keys WHERE key_id = $1")
            .bind(MARKER_KEY_ID)
            .fetch_optional(pool)
            .await
            .context("读取查找哈希版本标记失败")?;
    Ok(row.map(|(v,)| v))
}

/// 写回填标记（幂等）。
pub async fn write_marker(pool: &DbPool, version: i32) -> Result<()> {
    sqlx::query(
        "INSERT INTO encryption_keys (key_id, key_version, algorithm, status)
         VALUES ($1, $2, 'HMAC-SHA256', 'active')
         ON CONFLICT (key_id) DO UPDATE SET
             key_version = EXCLUDED.key_version,
             algorithm   = EXCLUDED.algorithm,
             rotated_at  = NOW()",
    )
    .bind(MARKER_KEY_ID)
    .bind(version)
    .execute(pool)
    .await
    .context("写入查找哈希版本标记失败")?;
    Ok(())
}

/// 回填报告
#[derive(Debug, Default)]
pub struct RehashReport {
    /// 扫过的行数（密文列非空的）
    pub scanned: usize,
    /// 已经是 v2、无需改动的
    pub already_current: usize,
    /// 真的被改写的
    pub rehashed: usize,
    /// 哈希列原本为 NULL、这次补上的
    pub filled_null: usize,
}

impl RehashReport {
    pub fn describe(&self) -> String {
        format!(
            "扫描 {} 行：已是 v2 {} 行、本次改写 {} 行、原本为空补上 {} 行",
            self.scanned, self.already_current, self.rehashed, self.filled_null
        )
    }
}

/// 一次性回填全部可回填的哈希列。
///
/// - `dry_run = true`：只统计、不写库（**默认**，见 `bin/rehash_lookup_columns.rs`）
/// - 解密失败**立即中止并返回错误**，不做「跳过这一行继续」——
///   解密失败只有一个原因：**MASTER_KEY 不对**。继续跑下去会把
///   「密钥不对」变成「部分行被改成了新哈希、部分没改」，
///   那是最难恢复的一种状态。
/// - 幂等：已经是新哈希的行会被识别出来并跳过，可以反复跑。
///
/// 每个 (表, 列) 组合一个事务 —— 中途失败时不会留下「一半新一半旧」的表。
pub async fn rehash_all(
    pool: &DbPool,
    encryptor: &Encryptor,
    dry_run: bool,
) -> Result<RehashReport> {
    let mut report = RehashReport::default();

    for (table, enc_col, hash_col) in REHASHABLE {
        let select_sql = format!(
            "SELECT id, {enc_col}, {hash_col} FROM {table} WHERE {enc_col} IS NOT NULL"
        );
        let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(&select_sql)
            .fetch_all(pool)
            .await
            .with_context(|| format!("读取 {table}.{enc_col} 失败"))?;

        report.scanned += rows.len();
        let mut pending: Vec<(String, Uuid)> = Vec::new();

        for (id, encrypted, stored) in rows {
            let plaintext = encryptor.decrypt(&encrypted).map_err(|e| {
                anyhow::anyhow!(
                    "解密 {table}.{enc_col}（id={id}）失败：{e} —— \
                     这几乎一定是 MASTER_KEY 不对。回填**已中止**，没有写入任何一行。\
                     请先用原来的 MASTER_KEY 再试。"
                )
            })?;

            let expected = EncryptedFieldsOps::generate_hash(&plaintext);
            match stored {
                Some(s) if s == expected => report.already_current += 1,
                Some(_) => {
                    report.rehashed += 1;
                    pending.push((expected, id));
                }
                None => {
                    report.filled_null += 1;
                    pending.push((expected, id));
                }
            }
        }

        if !dry_run && !pending.is_empty() {
            let update_sql = format!("UPDATE {table} SET {hash_col} = $1 WHERE id = $2");
            let mut tx = pool.begin().await?;
            for (hash, id) in &pending {
                sqlx::query(&update_sql)
                    .bind(hash)
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .with_context(|| format!("更新 {table}.{hash_col}（id={id}）失败"))?;
            }
            tx.commit().await.with_context(|| {
                format!("提交 {table}.{hash_col} 的回填事务失败 —— 该表已整表回滚")
            })?;
        }
    }

    Ok(report)
}

// ─────────────────────────── 启动守卫的判定 ───────────────────────────

/// 启动时检查「算法换了、存量行还是旧哈希」这件事该怎么了结。
#[derive(Debug, PartialEq, Eq)]
pub enum StartupDecision {
    /// 标记已是当前版本，放行
    Ok,
    /// 库里一行数据都没有 —— 全新部署。写入标记后放行，
    /// 免得第一次写入数据之后还要再跑一次回填。
    MarkFresh,
    /// 有数据、但标记不是当前版本 —— **拒绝启动**
    NeedsRehash,
}

/// 判定逻辑（纯函数，刻意与数据库访问分开，这样能直接单测）。
///
/// ⚠️ 为什么「有数据 + 无标记」不能默认放行：那正是从裸 SHA-256 升级上来的库
/// 的样子。放行的后果是**所有按哈希查行的路径都查不到任何行** ——
/// 用户拿着正确的卡密被告知不存在、登录说邮箱没注册过，而接口 200、日志干净。
/// 宁可拒绝启动并打出「请跑 rehash_lookup_columns」。
pub fn startup_decision(marker: Option<i32>, counts: &RowCounts) -> StartupDecision {
    if marker == Some(V2_HMAC_SHA256) {
        return StartupDecision::Ok;
    }
    if counts.total() == 0 {
        return StartupDecision::MarkFresh;
    }
    StartupDecision::NeedsRehash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(merchants: i64, cards: i64, activations: i64, blacklist: i64) -> RowCounts {
        RowCounts {
            merchants,
            cards,
            activations,
            device_blacklist: blacklist,
        }
    }

    #[test]
    fn empty_database_is_marked_fresh_not_refused() {
        // 全新部署必须能起来，否则第一次跑都跑不通。
        let c = counts(0, 0, 0, 0);
        assert_eq!(startup_decision(None, &c), StartupDecision::MarkFresh);
        assert_eq!(startup_decision(Some(V1_BARE_SHA256), &c), StartupDecision::MarkFresh);
    }

    #[test]
    fn data_without_marker_must_be_refused() {
        // 这就是「从裸 SHA-256 升级上来、还没跑回填」的样子 —— 必须拒绝启动。
        let c = counts(1, 0, 0, 0);
        assert_eq!(startup_decision(None, &c), StartupDecision::NeedsRehash);
        assert_eq!(
            startup_decision(Some(V1_BARE_SHA256), &c),
            StartupDecision::NeedsRehash
        );
    }

    #[test]
    fn current_marker_always_passes() {
        // 标记是 v2 时无论有没有数据都放行（回填已经跑过了）。
        assert_eq!(
            startup_decision(Some(V2_HMAC_SHA256), &counts(0, 0, 0, 0)),
            StartupDecision::Ok
        );
        assert_eq!(
            startup_decision(Some(V2_HMAC_SHA256), &counts(9, 9, 9, 9)),
            StartupDecision::Ok
        );
    }

    #[test]
    fn only_blacklist_rows_still_counts_as_having_data() {
        // device_blacklist 回填不了（没有明文可回收），但**有行**就说明
        // 这个库是旧算法时代建的 —— 依旧要走一次回填（那个脚本会跳过它、
        // 只写标记），否则连「黑名单里有没有旧哈希」都无从确认。
        let c = counts(0, 0, 0, 1);
        assert_eq!(c.total(), 1);
        assert_eq!(startup_decision(None, &c), StartupDecision::NeedsRehash);
    }

    #[test]
    fn unknown_future_version_is_not_treated_as_ok() {
        // 例如有人回滚到旧二进制：标记是 v3（更新的版本），
        // 当前代码不认识 → 不能假设它兼容，必须要求回填流程介入。
        let c = counts(1, 0, 0, 0);
        assert_eq!(startup_decision(Some(3), &c), StartupDecision::NeedsRehash);
    }
}
