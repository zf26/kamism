# 字段级加密集成指南

## 概述

本系统实现了企业级的字段级加密方案，使用 AES-256-GCM 算法加密敏感数据，支持密钥管理和轮换。

## 架构设计

### 加密流程

```
明文数据
   ↓
KMS 管理器（主密钥）
   ↓
派生数据加密密钥（DEK）
   ↓
AES-256-GCM 加密
   ↓
加密数据 + Nonce（十六进制编码）
   ↓
数据库存储
```

### 密钥管理

- **主密钥（Master Key）**：32 字节，从环境变量 `MASTER_KEY` 读取
- **数据加密密钥（DEK）**：通过 SHA-256 从主密钥派生，支持按字段/记录隔离
- **密钥版本**：使用 `key_id` 标识，支持密钥轮换

## 敏感字段清单

| 表名 | 字段 | 加密字段 | 用途 |
|------|------|---------|------|
| merchants | api_key | api_key_encrypted | API 密钥 |
| merchants | email | email_encrypted | 商户邮箱 |
| cards | code | code_encrypted | 卡密代码 |
| activations | device_id | device_id_encrypted | 设备标识 |

## 环境配置

### 1. 生成主密钥

```bash
# 生成 32 字节的随机主密钥（64 个十六进制字符）
MASTER_KEY=$(openssl rand -hex 32)
echo $MASTER_KEY
# 输出示例：a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5z6a7b8c9d0e1f2
```

### 2. 配置环境变量

在 `.env` 文件中添加：

```env
# 主密钥（必须是 64 个十六进制字符，代表 32 字节）
MASTER_KEY=a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5z6a7b8c9d0e1f2

# 其他配置...
DATABASE_URL=postgres://...
REDIS_URL=redis://...
```

### 3. 生产环境密钥管理

**推荐方案**：使用专业的密钥管理服务

#### AWS KMS

```rust
// 使用 AWS KMS 获取主密钥
use aws_sdk_kms::Client;

let client = Client::new(&config);
let response = client
    .decrypt()
    .ciphertext_blob(encrypted_key)
    .send()
    .await?;

let master_key = response.plaintext().unwrap();
```

#### Azure Key Vault

```rust
// 使用 Azure Key Vault 获取主密钥
use azure_identity::DefaultAzureCredential;
use azure_security_keyvault::KeyVaultClient;

let credential = DefaultAzureCredential::new()?;
let client = KeyVaultClient::new(&credential);
let secret = client.get_secret("master-key").await?;
```

#### HashiCorp Vault

```rust
// 使用 Vault 获取主密钥
use vaultrs::client::VaultClient;

let client = VaultClient::new()?;
let secret = client.read("secret/data/master-key").await?;
```

## 使用示例

### 创建商户时加密 API Key

```rust
use crate::db::encrypted_fields::EncryptedFieldsOps;

// 生成 API Key
let api_key = generate_api_key();

// 加密 API Key
let encrypted_api_key = EncryptedFieldsOps::encrypt_merchant_api_key(
    &state.pool,
    &state.encryptor,
    merchant_id,
    &api_key,
).await?;

// 存储到数据库
sqlx::query(
    "INSERT INTO merchants (id, api_key_encrypted, ...) VALUES ($1, $2, ...)"
)
.bind(merchant_id)
.bind(&encrypted_api_key)
.execute(&state.pool)
.await?;
```

### 查询商户时解密 API Key

```rust
// 查询加密的 API Key
let (encrypted_api_key,): (String,) = sqlx::query_as(
    "SELECT api_key_encrypted FROM merchants WHERE id = $1"
)
.bind(merchant_id)
.fetch_one(&state.pool)
.await?;

// 解密
let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
    &state.encryptor,
    &encrypted_api_key,
).await?;
```

### 生成卡密时加密代码

```rust
// 生成卡密代码
let card_code = generate_card_code();

// 加密卡密代码
let encrypted_code = EncryptedFieldsOps::encrypt_card_code(
    &state.pool,
    &state.encryptor,
    card_id,
    &card_code,
).await?;

// 存储到数据库
sqlx::query(
    "INSERT INTO cards (id, code_encrypted, ...) VALUES ($1, $2, ...)"
)
.bind(card_id)
.bind(&encrypted_code)
.execute(&state.pool)
.await?;
```

### 验证卡密时解密并比对

```rust
// 查询加密的卡密代码
let (encrypted_code,): (String,) = sqlx::query_as(
    "SELECT code_encrypted FROM cards WHERE id = $1"
)
.bind(card_id)
.fetch_one(&state.pool)
.await?;

// 解密
let stored_code = EncryptedFieldsOps::decrypt_card_code(
    &state.encryptor,
    &encrypted_code,
).await?;

// 比对用户输入
if stored_code == user_input_code {
    // 卡密有效
}
```

## 密钥轮换

### 轮换流程

1. **生成新主密钥**

```bash
NEW_MASTER_KEY=$(openssl rand -hex 32)
```

2. **更新环境变量**

```bash
# 更新 .env 或密钥管理服务
MASTER_KEY=$NEW_MASTER_KEY
```

3. **重新加密所有数据**

```rust
// 使用新主密钥重新加密所有敏感字段
pub async fn rotate_encryption_keys(pool: &PgPool, encryptor: &Encryptor) -> Result<()> {
    // 查询所有加密的 API Key
    let merchants: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, api_key_encrypted FROM merchants WHERE api_key_encrypted IS NOT NULL"
    )
    .fetch_all(pool)
    .await?;

    for (merchant_id, encrypted_api_key) in merchants {
        // 解密旧数据
        let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
            encryptor,
            &encrypted_api_key,
        ).await?;

        // 使用新密钥重新加密
        let new_encrypted = EncryptedFieldsOps::encrypt_merchant_api_key(
            pool,
            encryptor,
            merchant_id,
            &api_key,
        ).await?;

        // 更新数据库
        sqlx::query(
            "UPDATE merchants SET api_key_encrypted = $1 WHERE id = $2"
        )
        .bind(&new_encrypted)
        .bind(merchant_id)
        .execute(pool)
        .await?;
    }

    Ok(())
}
```

4. **验证和清理**

```bash
# 验证所有数据都已重新加密
SELECT COUNT(*) FROM merchants WHERE api_key_encrypted IS NULL;

# 备份旧主密钥（用于紧急恢复）
# 保存到安全的离线位置
```

## 性能优化

### 1. 缓存解密结果

```rust
// 在 Redis 中缓存解密的 API Key（TTL 1 小时）
let cache_key = format!("api_key:{}", merchant_id);

// 尝试从缓存读取
if let Ok(cached_api_key) = redis_conn.get::<_, String>(&cache_key).await {
    return Ok(cached_api_key);
}

// 缓存未命中，解密并缓存
let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
    &state.encryptor,
    &encrypted_api_key,
).await?;

redis_conn.set_ex(&cache_key, &api_key, 3600).await?;

Ok(api_key)
```

### 2. 批量加密/解密

```rust
// 批量加密卡密代码
let card_codes: Vec<(Uuid, String)> = vec![
    (card_id_1, "KAMI-XXXX-XXXX-XXXX-XXXX"),
    (card_id_2, "KAMI-YYYY-YYYY-YYYY-YYYY"),
];

let encrypted_codes: Vec<String> = futures::future::try_join_all(
    card_codes.iter().map(|(card_id, code)| {
        EncryptedFieldsOps::encrypt_card_code(
            &state.pool,
            &state.encryptor,
            *card_id,
            code,
        )
    })
).await?;
```

### 3. 异步处理

```rust
// 后台任务：批量加密历史数据
tokio::spawn(async move {
    if let Err(e) = encrypt_historical_data(&pool, &encryptor).await {
        tracing::error!("加密历史数据失败: {}", e);
    }
});
```

## 安全最佳实践

### 1. 访问控制

```rust
// 只有授权用户才能查看敏感字段
pub async fn get_merchant_api_key(
    State(state): State<AppState>,
    claims: Claims,
    Path(merchant_id): Path<Uuid>,
) -> Result<Json<ApiKeyResponse>> {
    // 验证权限
    if claims.merchant_id != merchant_id && claims.role != "admin" {
        return Err(AppError::Forbidden);
    }

    // 查询并解密
    let encrypted_api_key = get_encrypted_api_key(&state.pool, merchant_id).await?;
    let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
        &state.encryptor,
        &encrypted_api_key,
    ).await?;

    Ok(Json(ApiKeyResponse { api_key }))
}
```

### 2. 审计日志

```rust
// 记录所有敏感操作
pub async fn log_sensitive_operation(
    pool: &PgPool,
    user_id: Uuid,
    operation: &str,
    resource: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_logs (user_id, operation, resource, timestamp)
         VALUES ($1, $2, $3, NOW())"
    )
    .bind(user_id)
    .bind(operation)
    .bind(resource)
    .execute(pool)
    .await?;

    Ok(())
}
```

### 3. 日志脱敏

```rust
// 不要在日志中输出敏感数据
tracing::info!("加密 API Key: merchant_id={}", merchant_id);
// ✓ 正确

tracing::info!("加密 API Key: {}", api_key);
// ✗ 错误：泄露敏感数据
```

## 故障排查

### 问题 1：MASTER_KEY 格式错误

```
Error: MASTER_KEY 必须是 32 字节（64 个十六进制字符）
```

**解决方案**：

```bash
# 验证主密钥长度
echo -n "your_master_key" | wc -c
# 应该输出 64

# 重新生成
MASTER_KEY=$(openssl rand -hex 32)
```

### 问题 2：解密失败

```
Error: 解密失败: authentication tag mismatch
```

**原因**：
- 使用了错误的主密钥
- 加密数据被篡改
- 密钥版本不匹配

**解决方案**：
- 检查 MASTER_KEY 是否正确
- 验证数据库中的加密数据完整性
- 检查 key_id 是否匹配

### 问题 3：性能下降

**优化建议**：
- 启用 Redis 缓存
- 使用连接池
- 批量操作而不是逐条处理
- 考虑异步处理

## 测试

### 单元测试

```bash
cargo test --lib utils::kms
```

### 集成测试

```bash
# 启动测试数据库
docker-compose -f docker-compose.test.yml up

# 运行集成测试
cargo test --test '*' -- --test-threads=1
```

## 监控和告警

### 关键指标

- 加密/解密操作耗时
- 缓存命中率
- 密钥轮换状态
- 审计日志大小

### 告警规则

```yaml
# Prometheus 告警规则
- alert: EncryptionLatencyHigh
  expr: histogram_quantile(0.95, encryption_duration_seconds) > 0.1
  for: 5m
  annotations:
    summary: "加密操作延迟过高"

- alert: DecryptionFailures
  expr: rate(decryption_failures_total[5m]) > 0.01
  for: 5m
  annotations:
    summary: "解密失败率过高"
```

## 参考资源

- [AES-GCM 算法](https://en.wikipedia.org/wiki/Galois/Counter_Mode)
- [NIST 密钥管理指南](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-57pt1r5.pdf)
- [OWASP 密钥管理](https://cheatsheetseries.owasp.org/cheatsheets/Key_Management_Cheat_Sheet.html)

