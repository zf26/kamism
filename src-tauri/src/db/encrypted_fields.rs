use crate::utils::kms::Encryptor;
use anyhow::Result;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 加密字段操作模块
///
/// # 这个模块唯一要守住的不变量
///
/// **加密日志必须和它描述的业务行在同一个事务里写入。**
///
/// `encrypted_fields_log` 没有外键 —— `(table_name, record_id)` 是多态指向
/// （同一个 `record_id` 列分别指向 cards / activations / merchants），
/// 数据库层面建不出 FK，也就不会替我们维护这张表。日志一旦和业务行脱钩，
/// 会出两个方向的**静默**失败（都不报错、都不影响接口返回值）：
///
/// 1. **先写日志、后插业务行** → 业务 INSERT 失败时日志已经提交 → 孤儿。
///    实测过：库里 139 行日志**全部**是孤儿（卡密 46 + 激活 93），
///    推导过程写在 `migrations/010_purge_orphan_encryption_logs.sql` 里。
/// 2. **业务行先提交、日志后写**（尤其在 `tokio::spawn` 里写）→ 日志缺失。
///    失败只表现为一行 `error!`，调用方完全不知道；进程退出时任务还会被直接丢掉。
///    卡密批量生成曾经就是这种写法（`routes/cards.rs`）。
///
/// 所以本模块**只提供 `_tx` 变体**（接受调用方的事务）。
/// 曾经存在一组「自己拿连接池开短事务」的同名函数
/// （`log_encryption` / `encrypt_merchant_api_key` / `encrypt_card_code` /
/// `encrypt_device_id` / `encrypt_merchant_email`），已全部删除：
/// 它们的签名看起来更省事，但在真实调用路径上**每一个都是错的** ——
/// 删除路径上还有另一道保险（`migrations/010` 的 AFTER DELETE 触发器，
/// 覆盖含级联在内的所有删除）。
///
/// # 关于 `key_id`
///
/// 密文自带 key_id，格式 `key_id:nonce:ciphertext`（见 `utils/kms.rs` 的
/// `Encryptor::encrypt` / `decrypt`）：DEK = `SHA256(master_key || key_id)`，
/// 解密时从密文第一段**自取** key_id。也就是说这张日志表**不是解密所必需的**，
/// 它的作用是「按行反查用了哪个密钥版本」的审计索引。
/// （原先还有一个 `get_field_key_version()` 读取函数，全仓库无调用点，已删 ——
/// 免得后人以为密钥轮换依赖它。）
pub struct EncryptedFieldsOps;

impl EncryptedFieldsOps {
    /// 生成 SHA256 哈希值（用于精确匹配查询：按 api_key / 邮箱 / 卡密 / 设备号找行）
    ///
    /// ⚠️ 已知弱点（**尚未修，属遗留项**）：这是裸 SHA-256，没有服务端 pepper。
    /// 对低熵输入是实打实的风险 —— 卡密最短可以是 `seg_count=1, seg_len=2`，
    /// 字符集 32 个字符 → 只有 1024 种组合，拿到库的人可以离线枚举出全部哈希，
    /// 直接把 `code_hash` 还原成明文卡密。
    ///
    /// 正确修法是改成 HMAC-SHA256(pepper, value) + 一次性回填，但它会影响
    /// **所有按哈希查行的调用点（含删除路径**，删不到就等于静默失效），
    /// 必须单独一批做，不能顺手改。改之前请先看本文档头部的说明。
    ///
    /// ⚠️ 另一个不能顺手改的地方：输出**恒为 64 个小写十六进制字符**（纯 ASCII），
    /// 而有调用方依赖这个不变量做**按字节**切片 —— 见
    /// `routes/public_api.rs::verify_cache_key` 的 `&api_key_hash[..16]`。
    /// 按字节切纯 ASCII 是安全的（字节数 == 字符数），但如果哪天把这里的返回
    /// 换成「带前缀」或「非 ASCII」的表示，那些切片会**立刻**变成
    /// char-boundary panic（第十二批修的就是这个形态），而编译器不会提醒。
    pub fn generate_hash(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl EncryptedFieldsOps {
    /// 记录字段加密日志（**复用调用方的事务**）
    ///
    /// 为什么只有这一个入口：见模块头部说明。调用方如果还没开事务，
    /// 就自己 `pool.begin()` —— 别指望这里替你开一个，
    /// 「日志在事务外」正是本模块要消灭的东西。
    pub async fn log_encryption_tx<'e, E>(
        executor: E,
        table_name: &str,
        record_id: Uuid,
        field_name: &str,
        key_id: &str,
    ) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Self::log_encryption_inner(executor, table_name, record_id, field_name, key_id).await
    }

    /// 实际执行体：接受任意 Executor（`&mut PgConnection` 传 `&mut *tx`）
    async fn log_encryption_inner<'e, E>(
        executor: E,
        table_name: &str,
        record_id: Uuid,
        field_name: &str,
        key_id: &str,
    ) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
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
        .execute(executor)
        .await?;

        Ok(())
    }

    /// 加密商户 API Key（日志写进调用方的事务）
    ///
    /// `key_id` 约定为 `merchant_api_key_{merchant_id}`，
    /// 与 `routes/merchant.rs` 的手写实现保持一致（那里有更详细的说明）。
    pub async fn encrypt_merchant_api_key_tx<'e, E>(
        executor: E,
        encryptor: &Encryptor,
        merchant_id: Uuid,
        api_key: &str,
    ) -> Result<String>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let key_id = format!("merchant_api_key_{}", merchant_id);
        let encrypted = encryptor.encrypt(api_key, &key_id)?;

        Self::log_encryption_inner(executor, "merchants", merchant_id, "api_key", &key_id).await?;

        Ok(encrypted)
    }

    /// 解密商户 API Key
    pub fn decrypt_merchant_api_key(
        encryptor: &Encryptor,
        encrypted_api_key: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_api_key)
    }

    /// 加密商户邮箱（日志写进调用方的事务）
    pub async fn encrypt_merchant_email_tx<'e, E>(
        executor: E,
        encryptor: &Encryptor,
        merchant_id: Uuid,
        email: &str,
    ) -> Result<String>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let key_id = format!("merchant_email_{}", merchant_id);
        let encrypted = encryptor.encrypt(email, &key_id)?;

        Self::log_encryption_inner(executor, "merchants", merchant_id, "email", &key_id).await?;

        Ok(encrypted)
    }

    /// 解密商户邮箱
    pub fn decrypt_merchant_email(
        encryptor: &Encryptor,
        encrypted_email: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_email)
    }

    /// 加密设备 ID（日志写进调用方的事务）
    ///
    /// 加密本身是纯 CPU（`encryptor.encrypt` 只做 AEAD，不碰数据库），
    /// 唯一的数据库写入就是那条加密日志 —— 所以把日志绑进事务即可，
    /// 整个函数不需要第二条连接。
    pub async fn encrypt_device_id_tx<'e, E>(
        executor: E,
        encryptor: &Encryptor,
        activation_id: Uuid,
        device_id: &str,
    ) -> Result<String>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let key_id = format!("device_id_{}", activation_id);
        let encrypted = encryptor.encrypt(device_id, &key_id)?;

        Self::log_encryption_inner(executor, "activations", activation_id, "device_id", &key_id)
            .await?;

        Ok(encrypted)
    }

    /// 解密设备 ID
    pub fn decrypt_device_id(
        encryptor: &Encryptor,
        encrypted_device_id: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_device_id)
    }

    /// 解密卡密代码
    ///
    /// 注意：卡密的**加密**目前只发生在 `routes/cards.rs` 的批量生成里，
    /// 那里手写 key_id（`card_code_{预生成 uuid}`）并按同一事务写日志，
    /// 所以这里没有对应的 `encrypt_card_code_tx`。
    pub fn decrypt_card_code(
        encryptor: &Encryptor,
        encrypted_code: &str,
    ) -> Result<String> {
        encryptor.decrypt(encrypted_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_lowercase_hex() {
        // 确定性是硬要求：这些哈希列是用来做 `WHERE xxx_hash = $1` 精确匹配的，
        // 同输入两次不同结果就等于「查不到自己刚写进去的行」。
        let a = EncryptedFieldsOps::generate_hash("KAMI-ABCD-EFGH");
        let b = EncryptedFieldsOps::generate_hash("KAMI-ABCD-EFGH");
        assert_eq!(a, b);

        // 必须是小写十六进制，且长度固定 64。
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        // 不同输入必须不同（否则会命中别人的行）。
        assert_ne!(a, EncryptedFieldsOps::generate_hash("KAMI-ABCD-EFGI"));
    }

    /// 这条测试是**为了拦住一个具体的坑**：哈希列在库里是 `VARCHAR(64)`。
    ///
    /// 将来若把裸 SHA-256 换成带方案前缀的写法（比如为了治理「无 pepper」
    /// 而写成 `v2:<hex>` 或 `hmac-sha256:<hex>`），长度会立刻超过 64，
    /// INSERT/UPDATE 直接报「value too long for type character varying(64)」。
    /// 那个报错发生在写入路径上，是可见的 —— 但更糟的是**回填脚本**
    /// （改库、按行更新，通常不会像接口那样被盯着），很容易到很晚才发现。
    ///
    /// 所以这里把「哈希必须能塞进 VARCHAR(64)」钉成断言。
    /// 如果这条失败了，先去看 `migrations/001_init_complete.sql:25/27/56/73`
    /// 的列宽，两边必须一起改。
    #[test]
    fn hash_fits_into_varchar_64_columns() {
        // 取一个超长输入，确认输出长度与输入长度无关（SHA-256 特性）
        let long_input = "x".repeat(10_000);
        assert_eq!(EncryptedFieldsOps::generate_hash(&long_input).len(), 64);

        let column_width = 64usize; // merchants.api_key_hash / email_hash、cards.code_hash、activations.device_id_hash
        assert!(
            EncryptedFieldsOps::generate_hash("any").len() <= column_width,
            "哈希长度超过了 VARCHAR(64) 列宽，写入会失败"
        );
    }
}
