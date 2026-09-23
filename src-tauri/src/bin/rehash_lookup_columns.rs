//! 一次性回填：把**查找哈希**从裸 SHA-256（v1）换成 HMAC-SHA256(pepper)（v2）。
//!
//! ```bash
//! cargo run --bin rehash_lookup_columns              # 只统计，不写库（默认）
//! cargo run --bin rehash_lookup_columns -- --apply   # 真正写库
//! ```
//!
//! ── 为什么默认是 dry-run ────────────────────────────────────────────────
//!
//! 这个脚本会**改写每一条按哈希查行的记录**（商户 api_key / 邮箱、卡密、设备号）。
//! 跑错的后果不是「报错」，而是「所有查询都查不到行」。所以默认只统计，
//! 让人先看清楚「要动多少行」，再显式加 `--apply`。
//!
//! ── 什么时候必须跑 ──────────────────────────────────────────────────────
//!
//! 从裸 SHA-256 版本升级上来的库，**必须**跑一次并 `--apply`。
//! 没跑的话 `lib.rs::ensure_lookup_hash_version` 会在启动时**拒绝启动**
//! 并告诉你来跑这个脚本 —— 那是刻意的：把「所有查询静默失效」
//! 换成「启动时的一条明确指令」。
//!
//! ── 可以反复跑 ─────────────────────────────────────────────────────────
//!
//! 幂等。已经是 v2 的行会被识别出来跳过，不会重复改写。
//!
//! ── 跑不了的列（不是这个脚本的 bug）────────────────────────────────────
//!
//! `device_blacklist.device_id_hash` **无法回填**：那张表没有 `*_encrypted` 列，
//! 明文从来没落过库，哈希是不可逆的。运行时改为同时匹配新旧两种哈希
//! （见 `routes/public_api.rs` 的黑名单检查）。详见 `db/lookup_hash.rs` 头部说明。

use anyhow::{Context, Result};
use kamism_lib::db::lookup_hash::{self, V2_HMAC_SHA256};
use kamism_lib::db::{encrypted_fields, create_pool};
use kamism_lib::utils::kms::{Encryptor, KmsManager};
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    // 和服务器一样从 .env 读配置（workspace 根目录下跑即可）
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();

    let apply = env::args().any(|a| a == "--apply");

    let database_url = env::var("DATABASE_URL")
        .context("DATABASE_URL 未设置")?;
    let pool = create_pool(&database_url).await
        .context("连接数据库失败")?;

    let kms = KmsManager::new().context("KMS 初始化失败")?;

    // ⚠️ 顺序要紧：pepper 必须在任何 generate_hash 调用之前注入，
    // 否则会 panic（刻意的，见 encrypted_fields::lookup_pepper）。
    // 这里用 `&kms` 派生，然后再把它交给 Encryptor。
    encrypted_fields::init_lookup_pepper(kms.derive_lookup_pepper());
    tracing::info!(
        "lookup pepper 指纹: {}（必须与服务器启动日志里的那一个一致）",
        encrypted_fields::lookup_pepper_fingerprint()
    );

    let counts = lookup_hash::row_counts(&pool).await?;
    tracing::info!("当前行数: {}", counts.describe());

    // 没有 MASTER_KEY（本进程临时生成了一把）而库里又有数据 ——
    // 后面的解密必然全失败。这里提前给出明确原因，而不是让人去读
    // 一堆「解密 xxx 失败」。
    if kms.is_auto_generated() && counts.merchants > 0 {
        anyhow::bail!(
            "拒绝回填：当前进程没有可用的 MASTER_KEY（未配置，已临时生成一把新密钥），\
             而库里有 {} 个商户。用新密钥解密存量数据必然全部失败，\
             而失败的行拿不到明文、也就算不出新哈希。\
             请把原来的 MASTER_KEY 配到环境变量里再跑本脚本。",
            counts.merchants
        );
    }

    let marker = lookup_hash::read_marker(&pool).await?;
    tracing::info!("当前回填标记: {:?}", marker);

    let encryptor = Encryptor::new(kms);
    let report = lookup_hash::rehash_all(&pool, &encryptor, !apply).await?;

    if apply {
        lookup_hash::write_marker(&pool, V2_HMAC_SHA256).await?;
        tracing::info!("回填标记已写入 v{V2_HMAC_SHA256}");
    }

    println!();
    println!("================= rehash_lookup_columns =================");
    println!("模式: {}", if apply { "APPLY（已写库）" } else { "DRY-RUN（未写库）" });
    println!("{}", report.describe());
    if apply {
        println!("标记: v{V2_HMAC_SHA256}");
    } else {
        println!("确认无误后加 --apply 重新执行");
    }
    println!("注意: device_blacklist.device_id_hash 无法回填（无明文可回收），");
    println!("      运行时按新旧两种哈希同时匹配，见 routes/public_api.rs");
    println!("=========================================================");

    Ok(())
}
