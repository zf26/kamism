use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use hex::{decode, encode};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::env;

/// KMS 密钥管理器
/// 支持主密钥和数据加密密钥（DEK）的分离
pub struct KmsManager {
    /// 主密钥（Master Key），从环境变量读取或生成
    master_key: [u8; 32],
}

impl KmsManager {
    /// 初始化 KMS 管理器
    /// 优先级：环境变量 MASTER_KEY > 生成新密钥
    pub fn new() -> Result<Self> {
        let master_key = Self::load_or_generate_key()?;
        Ok(KmsManager { master_key })
    }

    /// 加载环境变量中的密钥，或自动生成并写入 .env
    fn load_or_generate_key() -> Result<[u8; 32]> {
        // 尝试从环境变量读取（空字符串视为未设置）
        if let Ok(key_hex) = env::var("MASTER_KEY") {
            let trimmed = key_hex.trim();
            if !trimmed.is_empty() {
                let key_hex = trimmed.trim_start_matches("0x");
                if let Ok(key_bytes) = decode(key_hex) {
                    if key_bytes.len() == 32 {
                        let mut key = [0u8; 32];
                        key.copy_from_slice(&key_bytes);
                        return Ok(key);
                    }
                }
                tracing::warn!("MASTER_KEY 格式无效（需要 64 位十六进制字符串），将自动生成新密钥");
            }
        }

        // 自动生成 32 字节随机密钥
        let mut rng = rand::thread_rng();
        let mut key = [0u8; 32];
        rng.fill(&mut key);
        let key_hex = encode(&key);
        tracing::info!("已自动生成 MASTER_KEY: {}", key_hex);

        // 尝试写入 .env 文件（可能不存在或只读，失败不影响启动）
        Self::persist_key_to_env(&key_hex);

        tracing::warn!(
            "请将上面的 MASTER_KEY 保存到 .env 文件中，否则重启后所有已加密的数据将无法解密！"
        );

        Ok(key)
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
                    tracing::info!("已自动写入 MASTER_KEY 到 {}", path);
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
}

