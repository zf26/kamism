/// 数据加密迁移脚本
/// 
/// 用于将现有的明文敏感字段加密
/// 
/// 使用方法：
/// ```bash
/// cargo run --bin encrypt_migration
/// ```

use anyhow::Result;
use kamism_lib::utils::kms::{Encryptor, KmsManager};
use sqlx::postgres::PgPoolOptions;
use std::env;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    // 读取环境变量
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost/kamism".to_string());

    // 创建数据库连接池
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&database_url)
        .await?;

    tracing::info!("已连接到数据库");

    // 初始化 KMS
    let kms = KmsManager::new()?;
    let encryptor = Encryptor::new(kms);

    tracing::info!("开始加密迁移...");

    // 1. 加密商户 API Key
    encrypt_merchant_api_keys(&pool, &encryptor).await?;

    // 2. 加密商户邮箱
    encrypt_merchant_emails(&pool, &encryptor).await?;

    // 3. 加密卡密代码
    encrypt_card_codes(&pool, &encryptor).await?;

    // 4. 加密设备 ID
    encrypt_device_ids(&pool, &encryptor).await?;

    tracing::info!("加密迁移完成！");

    Ok(())
}

/// 加密所有商户的 API Key
async fn encrypt_merchant_api_keys(pool: &sqlx::PgPool, encryptor: &Encryptor) -> Result<()> {
    tracing::info!("开始加密商户 API Key...");

    // 查询所有未加密的 API Key
    let merchants: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, api_key FROM merchants WHERE api_key_encrypted IS NULL"
    )
    .fetch_all(pool)
    .await?;

    tracing::info!("找到 {} 个未加密的商户 API Key", merchants.len());

    for (merchant_id, api_key) in merchants {
        match encrypt_merchant_api_key(pool, encryptor, merchant_id, &api_key).await {
            Ok(_) => {
                tracing::info!("已加密商户 {} 的 API Key", merchant_id);
            }
            Err(e) => {
                tracing::error!("加密商户 {} 的 API Key 失败: {}", merchant_id, e);
            }
        }
    }

    tracing::info!("商户 API Key 加密完成");
    Ok(())
}

/// 加密单个商户的 API Key
async fn encrypt_merchant_api_key(
    pool: &sqlx::PgPool,
    encryptor: &Encryptor,
    merchant_id: uuid::Uuid,
    api_key: &str,
) -> Result<()> {
    let key_id = format!("merchant_api_key_{}", merchant_id);
    let encrypted = encryptor.encrypt(api_key, &key_id)?;

    // 更新数据库
    sqlx::query(
        "UPDATE merchants SET api_key_encrypted = $1 WHERE id = $2"
    )
    .bind(&encrypted)
    .bind(merchant_id)
    .execute(pool)
    .await?;

    // 记录加密日志
    sqlx::query(
        "INSERT INTO encrypted_fields_log (table_name, record_id, field_name, key_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (table_name, record_id, field_name) DO UPDATE SET
         key_id = $4, encrypted_at = NOW()"
    )
    .bind("merchants")
    .bind(merchant_id)
    .bind("api_key")
    .bind(&key_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 加密所有商户的邮箱
async fn encrypt_merchant_emails(pool: &sqlx::PgPool, encryptor: &Encryptor) -> Result<()> {
    tracing::info!("开始加密商户邮箱...");

    let merchants: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, email FROM merchants WHERE email_encrypted IS NULL"
    )
    .fetch_all(pool)
    .await?;

    tracing::info!("找到 {} 个未加密的商户邮箱", merchants.len());

    for (merchant_id, email) in merchants {
        match encrypt_merchant_email(pool, encryptor, merchant_id, &email).await {
            Ok(_) => {
                tracing::info!("已加密商户 {} 的邮箱", merchant_id);
            }
            Err(e) => {
                tracing::error!("加密商户 {} 的邮箱失败: {}", merchant_id, e);
            }
        }
    }

    tracing::info!("商户邮箱加密完成");
    Ok(())
}

/// 加密单个商户的邮箱
async fn encrypt_merchant_email(
    pool: &sqlx::PgPool,
    encryptor: &Encryptor,
    merchant_id: uuid::Uuid,
    email: &str,
) -> Result<()> {
    let key_id = format!("merchant_email_{}", merchant_id);
    let encrypted = encryptor.encrypt(email, &key_id)?;

    sqlx::query(
        "UPDATE merchants SET email_encrypted = $1 WHERE id = $2"
    )
    .bind(&encrypted)
    .bind(merchant_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO encrypted_fields_log (table_name, record_id, field_name, key_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (table_name, record_id, field_name) DO UPDATE SET
         key_id = $4, encrypted_at = NOW()"
    )
    .bind("merchants")
    .bind(merchant_id)
    .bind("email")
    .bind(&key_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 加密所有卡密代码
async fn encrypt_card_codes(pool: &sqlx::PgPool, encryptor: &Encryptor) -> Result<()> {
    tracing::info!("开始加密卡密代码...");

    let cards: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, code FROM cards WHERE code_encrypted IS NULL"
    )
    .fetch_all(pool)
    .await?;

    tracing::info!("找到 {} 个未加密的卡密代码", cards.len());

    for (card_id, code) in cards {
        match encrypt_card_code(pool, encryptor, card_id, &code).await {
            Ok(_) => {
                tracing::info!("已加密卡密 {}", card_id);
            }
            Err(e) => {
                tracing::error!("加密卡密 {} 失败: {}", card_id, e);
            }
        }
    }

    tracing::info!("卡密代码加密完成");
    Ok(())
}

/// 加密单个卡密代码
async fn encrypt_card_code(
    pool: &sqlx::PgPool,
    encryptor: &Encryptor,
    card_id: uuid::Uuid,
    code: &str,
) -> Result<()> {
    let key_id = format!("card_code_{}", card_id);
    let encrypted = encryptor.encrypt(code, &key_id)?;

    sqlx::query(
        "UPDATE cards SET code_encrypted = $1 WHERE id = $2"
    )
    .bind(&encrypted)
    .bind(card_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO encrypted_fields_log (table_name, record_id, field_name, key_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (table_name, record_id, field_name) DO UPDATE SET
         key_id = $4, encrypted_at = NOW()"
    )
    .bind("cards")
    .bind(card_id)
    .bind("code")
    .bind(&key_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 加密所有设备 ID
async fn encrypt_device_ids(pool: &sqlx::PgPool, encryptor: &Encryptor) -> Result<()> {
    tracing::info!("开始加密设备 ID...");

    let activations: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, device_id FROM activations WHERE device_id_encrypted IS NULL"
    )
    .fetch_all(pool)
    .await?;

    tracing::info!("找到 {} 个未加密的设备 ID", activations.len());

    for (activation_id, device_id) in activations {
        match encrypt_device_id(pool, encryptor, activation_id, &device_id).await {
            Ok(_) => {
                tracing::info!("已加密设备 {}", activation_id);
            }
            Err(e) => {
                tracing::error!("加密设备 {} 失败: {}", activation_id, e);
            }
        }
    }

    tracing::info!("设备 ID 加密完成");
    Ok(())
}

/// 加密单个设备 ID
async fn encrypt_device_id(
    pool: &sqlx::PgPool,
    encryptor: &Encryptor,
    activation_id: uuid::Uuid,
    device_id: &str,
) -> Result<()> {
    let key_id = format!("device_id_{}", activation_id);
    let encrypted = encryptor.encrypt(device_id, &key_id)?;

    sqlx::query(
        "UPDATE activations SET device_id_encrypted = $1 WHERE id = $2"
    )
    .bind(&encrypted)
    .bind(activation_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO encrypted_fields_log (table_name, record_id, field_name, key_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (table_name, record_id, field_name) DO UPDATE SET
         key_id = $4, encrypted_at = NOW()"
    )
    .bind("activations")
    .bind(activation_id)
    .bind("device_id")
    .bind(&key_id)
    .execute(pool)
    .await?;

    Ok(())
}

