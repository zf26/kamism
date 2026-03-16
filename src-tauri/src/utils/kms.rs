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
        let master_key = if let Ok(key_hex) = env::var("MASTER_KEY") {
            // 从环境变量读取主密钥（必须是 64 个十六进制字符，代表 32 字节）
            let key_bytes = decode(&key_hex)
                .map_err(|_| anyhow!("MASTER_KEY 必须是有效的十六进制字符串（64 个字符）"))?;
            if key_bytes.len() != 32 {
                return Err(anyhow!("MASTER_KEY 必须是 32 字节（64 个十六进制字符）"));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&key_bytes);
            key
        } else {
            // 生成新的主密钥（仅用于开发环境）
            tracing::warn!("未设置 MASTER_KEY 环境变量，生成临时主密钥（仅用于开发）");
            let mut rng = rand::thread_rng();
            let mut key = [0u8; 32];
            rng.fill(&mut key);
            key
        };

        Ok(KmsManager { master_key })
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

