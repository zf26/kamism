use crate::utils::kms::Encryptor;
use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// 加密字段操作模块
pub struct EncryptedFieldsOps;

impl EncryptedFieldsOps {
    /// 生成 SHA256 哈希值（用于索引查询）
    pub fn generate_hash(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl EncryptedFieldsOps {
    /// 记录字段加密日志
    pub async fn log_encryption(
        pool: &PgPool,
        table_name: &str,
        record_id: Uuid,
        field_name: &str,
        key_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO encrypted_fields_log (table_name, record_id, field_name, key_id)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (table_name, record_id, field_name) DO UPDATE SET
             key_id = $4, encrypted_at = NOW()",
        )
        .bind(table_name)
        .bind(record_id)
        .bind(field_name)
        .bind(key_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// 获取字段的加密密钥版本
    pub async fn get_field_key_version(
        pool: &PgPool,
        table_name: &str,
        record_id: Uuid,
        field_name: &str,
    ) -> Result<Option<String>> {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT key_id FROM encrypted_fields_log
             WHERE table_name = $1 AND record_id = $2 AND field_name = $3",
        )
        .bind(table_name)
        .bind(record_id)
        .bind(field_name)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(|(key_id,)| key_id))
    }

    /// 加密商户 API Key
    pub async fn encrypt_merchant_api_key(
        pool: &PgPool,
        encryptor: &Encryptor,
        merchant_id: Uuid,
        api_key: &str,
    ) -> Result<String> {
        let key_id = format!("merchant_api_key_{}", merchant_id);
        let encrypted = encryptor.encrypt(api_key, &key_id)?;

        // 记录加密日志
        Self::log_encryption(pool, "merchants", merchant_id, "api_key", &key_id).await?;

        Ok(encrypted)
    }

    /// 解密商户 API Key
    pub fn decrypt_merchant_api_key(
        encryptor: &Encryptor,
        encrypted_api_key: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_api_key)
    }

    /// 加密卡密代码
    pub async fn encrypt_card_code(
        pool: &PgPool,
        encryptor: &Encryptor,
        card_id: Uuid,
        code: &str,
    ) -> Result<String> {
        let key_id = format!("card_code_{}", card_id);
        let encrypted = encryptor.encrypt(code, &key_id)?;

        // 记录加密日志
        Self::log_encryption(pool, "cards", card_id, "code", &key_id).await?;

        Ok(encrypted)
    }

    /// 解密卡密代码
    pub fn decrypt_card_code(
        encryptor: &Encryptor,
        encrypted_code: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_code)
    }

    /// 加密设备 ID
    pub async fn encrypt_device_id(
        pool: &PgPool,
        encryptor: &Encryptor,
        activation_id: Uuid,
        device_id: &str,
    ) -> Result<String> {
        let key_id = format!("device_id_{}", activation_id);
        let encrypted = encryptor.encrypt(device_id, &key_id)?;

        // 记录加密日志
        Self::log_encryption(pool, "activations", activation_id, "device_id", &key_id).await?;

        Ok(encrypted)
    }

    /// 解密设备 ID
    pub fn decrypt_device_id(
        encryptor: &Encryptor,
        encrypted_device_id: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_device_id)
    }

    /// 加密商户邮箱
    pub async fn encrypt_merchant_email(
        pool: &PgPool,
        encryptor: &Encryptor,
        merchant_id: Uuid,
        email: &str,
    ) -> Result<String> {
        let key_id = format!("merchant_email_{}", merchant_id);
        let encrypted = encryptor.encrypt(email, &key_id)?;

        // 记录加密日志
        Self::log_encryption(pool, "merchants", merchant_id, "email", &key_id).await?;

        Ok(encrypted)
    }

    /// 解密商户邮箱
    pub fn decrypt_merchant_email(
        encryptor: &Encryptor,
        encrypted_email: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_email)
    }
}

