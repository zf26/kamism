use crate::utils::kms::Encryptor;
use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use uuid::Uuid;

/// 查找哈希用的 pepper（进程内全局，启动时由 `lib.rs` 注入一次）
///
/// **为什么是全局而不是参数**：`generate_hash` 有 ~30 个调用点，全部散落在
/// 各个 handler 里。把它改成 `generate_hash(&state.kms, value)` 会把这批改动
/// 从「一个函数 + 一次回填」放大成「一次跨 8 个文件的大改」，而**改动面越大，
/// 越容易漏掉某一个查询点** —— 漏掉的那个点就是「静默查不到行」。
/// 用全局注入，签名保持不变，所有调用点自动获得新算法。
static LOOKUP_PEPPER: OnceLock<[u8; 32]> = OnceLock::new();

/// 启动时注入 pepper（由 `KmsManager::derive_lookup_pepper()` 派生）。
///
/// 重复注入同一把 pepper 是允许的（幂等）；注入不同的值是编程错误，
/// 会 panic —— 因为「一半的哈希用 A、一半用 B」在运行期是无法诊断的。
pub fn init_lookup_pepper(pepper: [u8; 32]) {
    if let Some(existing) = LOOKUP_PEPPER.get() {
        assert_eq!(
            existing, &pepper,
            "lookup pepper 被注入了两次不同的值 —— 这会让一部分哈希用 A、另一部分用 B"
        );
        return;
    }
    let _ = LOOKUP_PEPPER.set(pepper);
}

/// 取 pepper。未注入直接 panic（**不是**回退到裸 SHA-256）。
///
/// ⚠️ 刻意做成 panic 而不是静默回退：如果允许「没有 pepper 就用裸 SHA-256」，
/// 那么一次配置失误就会让所有写入悄悄退回到无 pepper 的弱哈希，
/// 而接口一切正常、测试全绿 —— 正是本项目一直在治的那种失效。
/// panic 会被 `CatchPanicLayer` 兜成 500（可见），
/// 而 `lib.rs` 的启动检查会在更早一步就拒绝启动。
fn lookup_pepper() -> &'static [u8; 32] {
    LOOKUP_PEPPER.get().unwrap_or_else(|| {
        panic!(
            "lookup pepper 未初始化：generate_hash 不能在 KMS 初始化之前调用。\
             生产路径请确认 lib.rs 在启动时调用了 init_lookup_pepper()"
        )
    })
}

/// pepper 指纹：`SHA256(pepper)` 的前 8 个十六进制字符。
///
/// **只用于启动日志与事后排查**（「这次部署和上次是不是同一把 pepper」），
/// 不能反过来推出 pepper（SHA-256 单向）。
pub fn lookup_pepper_fingerprint() -> String {
    let mut hasher = Sha256::new();
    hasher.update(lookup_pepper());
    let digest = format!("{:x}", hasher.finalize());
    digest[..8].to_string()
}

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
    /// 生成**查找哈希**（用于精确匹配查询：按 api_key / 邮箱 / 卡密 / 设备号找行）
    ///
    /// 算法：`HMAC-SHA256(lookup_pepper, value)`，输出 64 位小写十六进制。
    ///
    /// ── 为什么不是裸 SHA-256（本批改掉的东西） ────────────────────────────
    ///
    /// 这些列的输入**常常是低熵的**。最短的卡密可以是 `seg_count=1, seg_len=2`，
    /// 字符集 32 个字符 → 只有 **32² = 1024** 种组合。裸 SHA-256 是确定性的、
    /// 且没有密钥，所以拿到库的人枚举 1024 次就能把 `code_hash` 还原成明文卡密
    /// —— 旁边那条 `code_encrypted`（AES-256-GCM）**完全没被碰**。
    /// 一句话：**哈希列把加密废掉了。** 邮箱、设备号同理（都是可枚举的低熵输入）。
    ///
    /// HMAC 同样是确定性的（这是「按哈希查行」的硬要求），但引入了服务端密钥：
    /// 没有 pepper 就无法离线枚举，即使拿到完整的库。
    ///
    /// ── 为什么签名没变（这是本批敢动的关键） ──────────────────────────────
    ///
    /// 调用点全部调用 `generate_hash(value)`，pepper 走 `LOOKUP_PEPPER` 全局注入。
    /// 如果把签名改成 `generate_hash(&kms, value)`，改动面会从「一个函数 + 一次回填」
    /// 放大成 8 个文件的大改，而**改动面越大越容易漏掉某个查询点** ——
    /// 漏掉的那个点就是「静默查不到行」。
    ///
    /// ── 不能顺手改的地方 ────────────────────────────────────────────────
    ///
    /// 输出**恒为 64 个小写十六进制字符**（纯 ASCII），而有调用方依赖这个不变量
    /// 做**按字节**切片 —— 见 `routes/public_api.rs::verify_cache_key` 的
    /// `&api_key_hash[..16]`。按字节切纯 ASCII 是安全的（字节数 == 字符数），
    /// 但如果哪天把这里的返回换成「带前缀」或「非 ASCII」的表示
    /// （比如 `v2:<hex>`），那些切片会**立刻**变成 char-boundary panic
    /// （第十二批修的就是这个形态），而编译器不会提醒。
    /// 顺带一提：加前缀还会直接撑爆库里的 `VARCHAR(64)` 列
    /// （`migrations/001_init_complete.sql:25/27/56/73`），
    /// 单测 `hash_fits_into_varchar_64_columns` 钉住了这一条。
    pub fn generate_hash(value: &str) -> String {
        type HmacSha256 = Hmac<Sha256>;
        // HMAC 接受任意长度密钥，固定 32 字节的 pepper 不可能失败
        let mut mac = HmacSha256::new_from_slice(lookup_pepper())
            .expect("HMAC-SHA256 接受任意长度密钥");
        mac.update(value.as_bytes());
        format!("{:x}", mac.finalize().into_bytes())
    }

    /// **旧算法**（裸 SHA-256）—— 仅供 `device_blacklist` 的存量数据回退查询。
    ///
    /// ⚠️ **全仓库只有 1 个调用点**（`routes/public_api.rs` 的黑名单检查），
    /// 并且 E2E 用 `D4` 断言把这个数字钉死 —— 新增第二个调用点会立刻变红。
    ///
    /// 为什么它必须存在：`device_blacklist` 表**只有 `device_id_hash`，
    /// 没有对应的 `*_encrypted` 列**（见 `migrations/001_init_complete.sql`），
    /// 所以存量黑名单行的明文**已经无法找回**，`rehash_lookup_columns` 也回填不了。
    /// 如果切换算法后不再匹配旧哈希，那些被封的设备会**静默解封** ——
    /// 这是一种「不报错、恰恰相反地放行」的失效，比误封严重得多。
    ///
    /// 代价（记录在案，不假装没有）：这些存量黑名单行的哈希仍然是可枚举的。
    /// 缓解：黑名单的敏感度远低于卡密/邮箱，而且新写入的行一律走新算法，
    /// 存量行会随着管理员重新拉黑同一条设备号而自然被覆盖。
    pub fn generate_hash_legacy_sha256(value: &str) -> String {
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

    /// 单测进程里注入一把固定 pepper。
    ///
    /// 用**固定值**而不是随机值：单测要可重复（同输入同输出）。
    /// 这里钉的是「算法契约」，不是 pepper 的保密性 —— 保密性由
    /// `kms.rs` 的三条 pepper 契约测试负责。
    fn init_test_pepper() {
        init_lookup_pepper([0x42u8; 32]);
    }

    #[test]
    fn hash_is_deterministic_lowercase_hex() {
        init_test_pepper();

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
    /// 将来若把 HMAC 换成带方案前缀的写法（比如为了做算法版本迁移
    /// 而写成 `v2:<hex>` 或 `hmac-sha256:<hex>`），长度会立刻超过 64，
    /// INSERT/UPDATE 直接报「value too long for type character varying(64)」。
    /// 那个报错发生在写入路径上，是可见的 —— 但更糟的是**回填脚本**
    /// （改库、按行更新，通常不会像接口那样被盯着），很容易到很晚才发现。
    ///
    /// 所以这里把「两种算法的哈希都必须能塞进 VARCHAR(64)」钉成断言。
    /// 如果这条失败了，先去看 `migrations/001_init_complete.sql:25/27/56/73`
    /// 的列宽，两边必须一起改。
    #[test]
    fn hash_fits_into_varchar_64_columns() {
        init_test_pepper();

        // 取一个超长输入，确认输出长度与输入长度无关（HMAC-SHA256 特性）
        let long_input = "x".repeat(10_000);
        assert_eq!(EncryptedFieldsOps::generate_hash(&long_input).len(), 64);
        assert_eq!(
            EncryptedFieldsOps::generate_hash_legacy_sha256(&long_input).len(),
            64
        );

        let column_width = 64usize; // merchants.api_key_hash / email_hash、cards.code_hash、activations.device_id_hash
        for h in [
            EncryptedFieldsOps::generate_hash("any"),
            EncryptedFieldsOps::generate_hash_legacy_sha256("any"),
        ] {
            assert!(
                h.len() <= column_width,
                "哈希长度超过了 VARCHAR(64) 列宽，写入会失败"
            );
        }
    }

    // ── pepper 真的参与了运算 ────────────────────────────────────────────
    // 这是本批所有改动的「原子事实」：如果这一条不成立（比如 pepper 被忽略、
    // 或者新旧算法恰好对某个输入相同），那么上面的安全性说法全部不成立。

    #[test]
    fn keyed_hash_is_never_equal_to_the_legacy_bare_sha256() {
        init_test_pepper();

        for input in [
            "",
            "a",
            "KAMI-AB",
            "KAMI-ABCD-EFGH-IJKL-MNOP",
            "user@example.com",
            "设备号-中文",
            "km_0123456789abcdefghijklmnopqrstuv",
        ] {
            assert_ne!(
                EncryptedFieldsOps::generate_hash(input),
                EncryptedFieldsOps::generate_hash_legacy_sha256(input),
                "带 pepper 的哈希与裸 SHA-256 撞上了（input={:?}）—— pepper 没有参与运算？",
                input
            );
        }
    }

    /// **旧算法确实是标准 SHA-256** —— 用一个公开测试向量钉住。
    ///
    /// 为什么需要这条：`generate_hash_legacy_sha256` 的存在意义是「和库里
    /// 存量行的哈希逐字节一样」。如果有人哪天"顺手"把它也改成带 pepper 的，
    /// 存量黑名单会**静默失配**，而 E2E 的 1024 次枚举也解释不通了。
    /// 有了这个向量，改动会立刻撞在断言上。
    #[test]
    fn legacy_hash_is_plain_sha256_by_definition() {
        // 公开测试向量：SHA-256("abc")
        assert_eq!(
            EncryptedFieldsOps::generate_hash_legacy_sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// 把 E2E 里那个「1024 次枚举」在单测里做一遍（小规模版本）。
    ///
    /// 场景：最短格式的卡密 = 1 段 × 2 字符 = 32² = 1024 种组合。
    /// - 库里存的是裸 SHA-256 → 枚举能命中（还原出明文）
    /// - 库里存的是带 pepper 的 HMAC → 枚举命中不了
    ///
    /// 这条测试就是本批的**可执行论点**，不依赖数据库、不依赖网络。
    ///
    /// ⚠️ 关键细节（第一版写错过）：攻击者手里只有**已经泄露的那一个哈希值**，
    /// 而他**只会算裸 SHA-256**（没有 pepper）。所以枚举必须拿一个**固定的 target**
    /// 去比。第一版写成 `hash_fn(candidate) == hash_fn(secret)` —— 候选恰好等于
    /// 真值时这条**恒等成立**，于是「HMAC 还原不出来」那句断言变成同义反复，
    /// 测试红了才发现：它证明的不是「pepper 有用」，而是「我比较的是同一个数」。
    /// 这类错误在真实 E2E 里就是「枚举看似有效、其实什么都没验」。
    #[test]
    fn short_card_code_is_enumerable_only_without_pepper() {
        init_test_pepper();

        const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

        // 被"泄露"的那张卡密（商户把 seg_count/seg_len 都调到最小值）
        let secret = "KAMI-2A";

        // 攻击者的能力：只会算裸 SHA-256（他没有 pepper）
        //
        // 枚举器接收两个参数：(攻击者能算的哈希函数, 他手上的目标哈希)。
        // 这样写才能表达真实的攻击模型 —— 固定 target、换函数，而不是
        // 每次都拿 secret 现算一个 target（那会退化成同义反复）。
        let crack = |hash_fn: &dyn Fn(&str) -> String, target: &str| -> Option<String> {
            for &a in CHARSET {
                for &b in CHARSET {
                    let candidate = format!("KAMI-{}{}", a as char, b as char);
                    if hash_fn(&candidate) == target {
                        return Some(candidate);
                    }
                }
            }
            None
        };
        let attacker = &(EncryptedFieldsOps::generate_hash_legacy_sha256
            as fn(&str) -> String);

        // 库里存的是**裸 SHA-256** → 100% 会被还原出明文
        let leaked_legacy = EncryptedFieldsOps::generate_hash_legacy_sha256(secret);
        assert_eq!(
            crack(attacker, &leaked_legacy).as_deref(),
            Some(secret),
            "裸 SHA-256 下应当能从哈希还原出明文卡密（这就是本批修的问题）"
        );

        // 库里存的是**带 pepper 的 HMAC**、而攻击者只会裸 SHA-256 → 命中不了
        let leaked_keyed = EncryptedFieldsOps::generate_hash(secret);
        assert_eq!(
            crack(attacker, &leaked_keyed),
            None,
            "带 pepper 的哈希不应被无密钥的枚举还原"
        );

        // 对照组：把带 pepper 的哈希函数交给枚举器，它立刻又能还原 ——
        // 证明上面那次「还原不了」是因为**缺 pepper**，
        // 而不是因为枚举器算错了（否则这条也会失败）。
        let keyed = &(EncryptedFieldsOps::generate_hash as fn(&str) -> String);
        assert_eq!(
            crack(keyed, &leaked_keyed).as_deref(),
            Some(secret),
            "拿到 pepper 之后同一套枚举应当能命中 —— 否则说明枚举器本身有问题"
        );

        // 而且枚举空间确实只有 1024 —— 这是"问题成立"的前提，
        // 若哪天把最短卡密改长了，这条会提醒我们重新评估风险等级。
        assert_eq!(CHARSET.len() * CHARSET.len(), 1024);
    }
}
