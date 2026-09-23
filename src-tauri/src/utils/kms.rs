use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use hex::{decode, encode};
use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::env;

/// KMS 密钥管理器
/// 支持主密钥和数据加密密钥（DEK）的分离
pub struct KmsManager {
    /// 主密钥（Master Key），从环境变量读取或生成
    master_key: [u8; 32],
    /// 本进程是否「临时生成」了主密钥（即没有可用配置）
    /// 启动流程用它判断「库里有加密数据、但密钥是新的」这种数据损坏场景
    auto_generated: bool,
}

/// 解析 MASTER_KEY 文本
///
/// - `Ok(None)`：未配置或空串 —— 首次安装，允许自动生成
/// - `Ok(Some(key))`：合法的 32 字节密钥
/// - `Err(..)`：**配置了但非法** —— 调用方必须拒绝启动
///
/// 为什么「非法」要拒绝启动，而不是像以前那样"自动生成一把新的继续跑"：
/// 换密钥等于已加密字段（api_key / 邮箱 / 卡密 / 设备 ID）全部解不开，而这属于
/// 静默失败 —— 服务照常启动、接口照常 200，只是数据永远读不回来。相比之下
/// 启动失败是可见、可修的。所以：非法就是错误，不做兜底。
pub fn parse_master_key(raw: Option<&str>) -> Result<Option<[u8; 32]>> {
    let raw = match raw {
        Some(r) => r,
        None => return Ok(None),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let hex_part = trimmed
        .trim_start_matches("0x")
        .trim_start_matches("0X");

    let bytes = decode(hex_part).map_err(|e| {
        anyhow!(
            "MASTER_KEY 不是合法的十六进制字符串（{}）。\
             请用 `openssl rand -hex 32` 生成 64 位十六进制密钥；\
             注意不要保留 env.example 里的中文占位说明。当前值长度 {} 字符。",
            e,
            trimmed.chars().count()
        )
    })?;

    if bytes.len() != 32 {
        return Err(anyhow!(
            "MASTER_KEY 长度错误：需要 32 字节（64 个十六进制字符），实际 {} 字节（{} 个字符）",
            bytes.len(),
            hex_part.chars().count()
        ));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(Some(key))
}

impl KmsManager {
    /// 初始化 KMS 管理器
    pub fn new() -> Result<Self> {
        let (master_key, auto_generated) = Self::load_or_generate_key()?;
        Ok(KmsManager {
            master_key,
            auto_generated,
        })
    }

    /// 本进程的主密钥是否来自「临时生成」（而非显式配置）
    pub fn is_auto_generated(&self) -> bool {
        self.auto_generated
    }

    /// 加载环境变量中的密钥；未配置时自动生成并尽量持久化
    /// 返回 (密钥, 是否为临时生成)
    fn load_or_generate_key() -> Result<([u8; 32], bool)> {
        if let Some(key) = parse_master_key(env::var("MASTER_KEY").ok().as_deref())? {
            tracing::info!("MASTER_KEY 已从环境变量加载（32 字节）");
            return Ok((key, false));
        }

        // 未配置：生成一把临时密钥（首次安装可直接跑起来）
        let mut rng = rand::thread_rng();
        let mut key = [0u8; 32];
        rng.fill(&mut key);
        let key_hex = encode(&key);

        // 测试进程不落盘：避免 `cargo test` 在仓库里留下一个随机 .env
        if !cfg!(test) {
            Self::persist_key_to_env(&key_hex);
        }

        tracing::warn!(
            "未配置 MASTER_KEY，本进程已临时生成一把：{} —— 请立即写入环境变量（容器部署请写进 compose 的 environment）。\
             否则进程重启（尤其是容器重建）后，所有已加密字段都无法解密。",
            key_hex
        );

        Ok((key, true))
    }

    /// 将密钥写入 .env 文件（幂等，失败静默）
    fn persist_key_to_env(key_hex: &str) {
        let env_paths = [".env", ".env.production", ".env.development"];
        for path in &env_paths {
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // 文件不存在，尝试下一个
            };

            let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            let key_line = format!("MASTER_KEY={}", key_hex);

            // 检查是否已有 MASTER_KEY 行
            let existing = lines.iter().position(|l| l.starts_with("MASTER_KEY="));
            match existing {
                Some(idx) => {
                    // 如果已有但内容不同则更新
                    if lines[idx] != key_line {
                        lines[idx] = key_line.clone();
                    } else {
                        return; // 内容一致，无需写入
                    }
                }
                None => {
                    // 没有 MASTER_KEY 行，追加
                    lines.push(String::new());
                    lines.push("# 主密钥（请勿泄露、勿修改，否则已加密数据无法解密）".to_string());
                    lines.push(key_line.clone());
                }
            }

            let new_content = lines.join("\n") + "\n";
            match std::fs::write(path, &new_content) {
                Ok(_) => {
                    tracing::info!(
                        "已自动写入 MASTER_KEY 到 {}（注意：容器内的 .env 会随容器重建丢失，\
                         容器部署请改用 compose 的 environment / 宿主机的 .env 传入）",
                        path
                    );
                    return; // 成功写入一个即可
                }
                Err(e) => {
                    tracing::warn!("写入 {} 失败（{}），请手动添加 MASTER_KEY", path, e);
                }
            }
        }

        // 所有文件都不存在，创建 .env
        let content = format!(
            "# 主密钥（请勿泄露、勿修改，否则已加密数据无法解密）\nMASTER_KEY={}\n",
            key_hex
        );
        if let Err(e) = std::fs::write(".env", &content) {
            tracing::warn!("创建 .env 文件失败（{}），请手动添加 MASTER_KEY", e);
        } else {
            tracing::info!("已创建 .env 文件并写入 MASTER_KEY");
        }
    }

    /// 生成数据加密密钥（DEK）
    /// 使用主密钥派生，支持密钥轮换
    pub fn derive_dek(&self, key_id: &str) -> Result<[u8; 32]> {
        let mut hasher = Sha256::new();
        hasher.update(&self.master_key);
        hasher.update(key_id.as_bytes());
        let result = hasher.finalize();

        let mut dek = [0u8; 32];
        dek.copy_from_slice(&result);
        Ok(dek)
    }

    /// 获取主密钥的十六进制表示（用于初始化或备份）
    pub fn get_master_key_hex(&self) -> String {
        encode(&self.master_key)
    }

    /// 派生「查找哈希」用的 pepper（给 `EncryptedFieldsOps::generate_hash` 用）
    ///
    /// ── 为什么查找哈希需要 pepper ──────────────────────────────────────────
    ///
    /// 那些 `*_hash` 列的作用是精确匹配（`WHERE api_key_hash = $1`），
    /// 所以**必须确定性**（同输入同输出），用不了随机 salt。确定性 + 无 pepper
    /// 就等于：拿到库的人可以对任意候选值离线算哈希。而这些列的输入常常是低熵的
    /// —— 最短的卡密只有 32² = 1024 种组合，枚举 1024 次就能把 `code_hash`
    /// **还原成明文卡密**，而旁边那条 `code_encrypted`（AES-256-GCM）根本没被碰。
    /// 换句话说：**哈希列把加密废掉了**。
    ///
    /// ── 为什么不能直接复用 `derive_dek` ─────────────────────────────────
    ///
    /// `derive_dek(key_id)` 是 `SHA256(master_key || key_id)`。如果这里也写成
    /// `derive_dek("lookup-hash-pepper")`，两条派生路径就是**同一个构造**
    /// （只是字符串常量不同），一旦哪天有人把某个 `key_id` 取成相同的串，
    /// 同一个 32 字节输出就会同时充当「加密密钥」和「哈希 pepper」。
    /// 所以这里换的是**构造本身** —— `HMAC-SHA256(master_key, label)`，
    /// 不只是换个标签字符串，而是结构性地区分开。
    ///
    /// ── 副作用，必须知道 ────────────────────────────────────────────────
    ///
    /// **换 MASTER_KEY = 换 pepper = 所有按哈希查行的路径一行都查不到。**
    /// 这与「换 MASTER_KEY = 所有加密字段解不开」是同一把密钥的两个后果，
    /// 所以这里没有引入新的失效场景，只是把既有的约束扩大到了哈希列。
    ///
    /// ⚠️ 但**别指望启动守卫替你把这件事挡住**。`lib.rs::ensure_master_key_matches_data`
    /// 只在 `kms.is_auto_generated()`（进程**没有配** MASTER_KEY、临时生成了一把）时
    /// 才生效，否则第一行就 `return Ok(())`。也就是说：
    /// **配了另一把 MASTER_KEY**（最常见的轮换场景）它拦不住 —— 启动会正常通过，
    /// 直到有请求去读加密字段、或按哈希查行时才暴露出来。
    /// 轮换密钥前请自觉确认库里有没有数据。
    pub fn derive_lookup_pepper(&self) -> [u8; 32] {
        type HmacSha256 = Hmac<Sha256>;
        // ⚠️ 必须写成 `<HmacSha256 as Mac>::new_from_slice` —— 本文件顶部
        // `use aes_gcm::aead::KeyInit` 也提供了同名方法，直接写
        // `HmacSha256::new_from_slice(...)` 会因为「两个 trait 都在作用域内」而编译失败。
        // HMAC 接受任意长度密钥，这里密钥恒为 32 字节，不可能失败
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.master_key)
            .expect("HMAC-SHA256 接受任意长度密钥");
        mac.update(b"kamism/lookup-hash-pepper/v1");
        let out = mac.finalize().into_bytes();

        let mut pepper = [0u8; 32];
        pepper.copy_from_slice(&out);
        pepper
    }
}

/// 加密器：处理字段级加密和解密
pub struct Encryptor {
    kms: KmsManager,
}

impl Encryptor {
    pub fn new(kms: KmsManager) -> Self {
        Encryptor { kms }
    }

    /// 加密敏感字段
    /// 返回格式：key_id:nonce:ciphertext（十六进制编码）
    pub fn encrypt(&self, plaintext: &str, key_id: &str) -> Result<String> {
        let dek = self.kms.derive_dek(key_id)?;
        let cipher = Aes256Gcm::new((&dek).into());

        // 生成随机 nonce（96 位）
        let mut rng = rand::thread_rng();
        let mut nonce_bytes = [0u8; 12];
        rng.fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // 加密
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow!("加密失败: {}", e))?;

        // 返回格式：key_id:nonce:ciphertext
        let result = format!(
            "{}:{}:{}",
            key_id,
            encode(&nonce_bytes),
            encode(&ciphertext)
        );

        Ok(result)
    }

    /// 解密敏感字段
    /// 输入格式：key_id:nonce:ciphertext（十六进制编码）
    pub fn decrypt(&self, encrypted: &str) -> Result<String> {
        let parts: Vec<&str> = encrypted.split(':').collect();
        if parts.len() != 3 {
            return Err(anyhow!("加密数据格式错误，应为 key_id:nonce:ciphertext"));
        }

        let key_id = parts[0];
        let nonce_hex = parts[1];
        let ciphertext_hex = parts[2];

        let dek = self.kms.derive_dek(key_id)?;
        let cipher = Aes256Gcm::new((&dek).into());

        let nonce_bytes = decode(nonce_hex)
            .map_err(|_| anyhow!("无效的 nonce 十六进制编码"))?;
        if nonce_bytes.len() != 12 {
            return Err(anyhow!("nonce 长度必须是 12 字节"));
        }
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = decode(ciphertext_hex)
            .map_err(|_| anyhow!("无效的密文十六进制编码"))?;

        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| anyhow!("解密失败: {}", e))?;

        String::from_utf8(plaintext).map_err(|e| anyhow!("解密结果不是有效的 UTF-8: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let kms = KmsManager::new().unwrap();
        let encryptor = Encryptor::new(kms);

        let plaintext = "sensitive_data_12345";
        let key_id = "merchant_api_key";

        let encrypted = encryptor.encrypt(plaintext, key_id).unwrap();
        println!("Encrypted: {}", encrypted);

        let decrypted = encryptor.decrypt(&encrypted).unwrap();
        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_different_key_ids() {
        let kms = KmsManager::new().unwrap();
        let encryptor = Encryptor::new(kms);

        let plaintext = "test_data";
        let encrypted1 = encryptor.encrypt(plaintext, "key_id_1").unwrap();
        let encrypted2 = encryptor.encrypt(plaintext, "key_id_2").unwrap();

        // 不同的 key_id 应该产生不同的密文
        assert_ne!(encrypted1, encrypted2);

        // 但都能正确解密
        assert_eq!(plaintext, encryptor.decrypt(&encrypted1).unwrap());
        assert_eq!(plaintext, encryptor.decrypt(&encrypted2).unwrap());
    }

    #[test]
    fn master_key_absent_means_first_install() {
        assert!(parse_master_key(None).unwrap().is_none());
        assert!(parse_master_key(Some("")).unwrap().is_none());
        assert!(parse_master_key(Some("   ")).unwrap().is_none());
    }

    #[test]
    fn master_key_accepts_common_valid_forms() {
        let hex = "0f".repeat(32);

        let key = parse_master_key(Some(&hex)).unwrap().unwrap();
        assert_eq!(key, [0x0fu8; 32]);

        // 0x 前缀、大写、两端空白都应被接受（复制粘贴时很常见）
        let messy = format!("  0x{}  ", hex.to_uppercase());
        assert_eq!(parse_master_key(Some(&messy)).unwrap().unwrap(), key);
    }

    #[test]
    fn master_key_invalid_forms_must_be_rejected() {
        // 这条就是 env.example 里那个中文占位串：`cp env.example .env` 之后
        // 最常见的一种配置错误。旧实现会静默换一把新密钥继续启动，
        // 结果所有已加密数据永久解不开 —— 现在必须直接报错。
        assert!(parse_master_key(Some("十六进制字符串（64 个字符）")).is_err());
        // 长度错
        assert!(parse_master_key(Some(&"a".repeat(63))).is_err());
        assert!(parse_master_key(Some(&"a".repeat(65))).is_err());
        assert!(parse_master_key(Some("abcdef")).is_err());
        // 非法字符
        assert!(parse_master_key(Some(&"z".repeat(64))).is_err());
    }

    /// 换一把主密钥，同一明文应产出不同密文并且互相解不开（确认密钥真的参与运算）
    #[test]
    fn different_master_keys_produce_incompatible_ciphertext() {
        let key_a = KmsManager {
            master_key: [1u8; 32],
            auto_generated: false,
        };
        let key_b = KmsManager {
            master_key: [2u8; 32],
            auto_generated: false,
        };
        let enc_a = Encryptor::new(key_a);
        let enc_b = Encryptor::new(key_b);

        let ciphertext = enc_a.encrypt("secret", "card_code_x").unwrap();
        assert_eq!(enc_a.decrypt(&ciphertext).unwrap(), "secret");
        assert!(enc_b.decrypt(&ciphertext).is_err());
    }

    // ── lookup pepper 的三条契约 ─────────────────────────────────────────
    // 这个值决定「能不能查到库里那些按哈希存的行」，所以三条都要钉住：
    // 稳定（否则查不到自己刚写的行）、随主密钥变（否则不是密钥材料）、
    // 且不与任何 DEK 重合（否则同一个 32 字节同时是加密密钥和哈希 pepper）。

    fn kms_with(byte: u8) -> KmsManager {
        KmsManager {
            master_key: [byte; 32],
            auto_generated: false,
        }
    }

    #[test]
    fn lookup_pepper_is_stable_for_the_same_master_key() {
        // 同一把主密钥两次派生必须一致 —— 这是「按哈希查行」能工作的前提。
        assert_eq!(
            kms_with(7).derive_lookup_pepper(),
            kms_with(7).derive_lookup_pepper()
        );
    }

    #[test]
    fn lookup_pepper_changes_with_the_master_key() {
        // 换主密钥必须换 pepper，否则 pepper 就不是密钥材料，离线枚举者可以
        // 完全忽略主密钥直接算哈希。
        assert_ne!(
            kms_with(7).derive_lookup_pepper(),
            kms_with(8).derive_lookup_pepper()
        );
    }

    /// 这条是**针对一个具体的未来误用**：如果哪天有人把 pepper 改成
    /// `derive_dek("lookup-hash-pepper")`（看起来更省事、也「更对称」），
    /// 两条派生路径就变成同一个构造，同一个 32 字节会同时充当
    /// 加密密钥与哈希 pepper。这里把「两者永不相等」钉成断言。
    #[test]
    fn lookup_pepper_never_collides_with_a_dek() {
        let kms = kms_with(7);
        let pepper = kms.derive_lookup_pepper();

        for key_id in [
            "lookup-hash-pepper",
            "kamism/lookup-hash-pepper/v1",
            "card_code_00000000-0000-0000-0000-000000000000",
            "merchant_api_key_00000000-0000-0000-0000-000000000000",
            "",
        ] {
            assert_ne!(
                pepper,
                kms.derive_dek(key_id).unwrap(),
                "pepper 与 derive_dek({:?}) 撞上了 —— 说明两条派生路径用了同一个构造",
                key_id
            );
        }
    }
}

