# 字段级加密实现说明

## 快速开始

### 1. 生成主密钥

```bash
# 生成 32 字节的随机主密钥
MASTER_KEY=$(openssl rand -hex 32)
echo "MASTER_KEY=$MASTER_KEY" >> .env
```

### 2. 配置环境变量

在 `.env` 中添加：

```env
MASTER_KEY=your_generated_master_key_here
```

### 3. 运行数据库迁移

```bash
# 迁移会自动创建加密相关的表
cargo sqlx migrate run
```

### 4. 迁移现有数据

如果你已有未加密的数据，运行迁移脚本：

```bash
cargo run --bin encrypt_migration
```

## 文件结构

```
src-tauri/
├── src/
│   ├── utils/
│   │   ├── kms.rs                    # KMS 密钥管理器和加密器
│   │   ├── encryption_examples.rs    # 使用示例和最佳实践
│   │   └── mod.rs                    # 导出 kms 模块
│   ├── db/
│   │   ├── encrypted_fields.rs       # 加密字段操作
│   │   └── mod.rs                    # 导出 encrypted_fields 模块
│   ├── middleware/
│   │   └── auth.rs                   # AppState 中添加 encryptor
│   └── lib.rs                        # 初始化 KMS 和 Encryptor
├── migrations/
│   └── 007_encryption.sql            # 加密表和字段
└── src/bin/
    └── encrypt_migration.rs          # 数据迁移脚本
```

## 核心模块

### 1. KMS 管理器 (`utils/kms.rs`)

**功能**：
- 从环境变量读取主密钥
- 派生数据加密密钥（DEK）
- 支持密钥轮换

**使用**：

```rust
use crate::utils::kms::{KmsManager, Encryptor};

// 初始化 KMS
let kms = KmsManager::new()?;
let encryptor = Encryptor::new(kms);

// 加密
let encrypted = encryptor.encrypt("sensitive_data", "key_id")?;

// 解密
let plaintext = encryptor.decrypt(&encrypted)?;
```

### 2. 加密字段操作 (`db/encrypted_fields.rs`)

**功能**：
- 加密/解密各类敏感字段
- 记录加密日志
- 支持字段级密钥隔离

**支持的字段**：
- `merchants.api_key` → `api_key_encrypted`
- `merchants.email` → `email_encrypted`
- `cards.code` → `code_encrypted`
- `activations.device_id` → `device_id_encrypted`

**使用**：

```rust
use crate::db::encrypted_fields::EncryptedFieldsOps;

// 加密 API Key
let encrypted = EncryptedFieldsOps::encrypt_merchant_api_key(
    &state.pool,
    &state.encryptor,
    merchant_id,
    "km_xxxxxxxx",
).await?;

// 解密 API Key
let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
    &state.encryptor,
    &encrypted,
).await?;
```

## 加密流程

### 创建商户时

```
1. 生成 API Key
2. 调用 encrypt_merchant_api_key()
3. 存储加密后的 API Key 到 api_key_encrypted 字段
4. 记录加密日志到 encrypted_fields_log 表
```

### 查询商户时

```
1. 从数据库查询 api_key_encrypted 字段
2. 调用 decrypt_merchant_api_key()
3. 返回解密后的 API Key
```

### 验证卡密时

```
1. 从数据库查询 code_encrypted 字段
2. 调用 decrypt_card_code()
3. 比对用户输入的卡密代码
```

## 密钥管理

### 主密钥存储

**开发环境**：
```env
MASTER_KEY=a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5z6a7b8c9d0e1f2
```

**生产环境**（推荐）：
- AWS KMS
- Azure Key Vault
- HashiCorp Vault
- 其他专业密钥管理服务

### 密钥轮换

1. 生成新主密钥
2. 更新环境变量
3. 运行迁移脚本重新加密所有数据
4. 备份旧主密钥（用于紧急恢复）

## 性能考虑

### 1. 缓存解密结果

对于频繁访问的字段，在 Redis 中缓存解密结果：

```rust
let cache_key = format!("api_key:{}", merchant_id);

// 尝试从缓存读取
if let Ok(cached) = redis_conn.get::<_, String>(&cache_key).await {
    return Ok(cached);
}

// 缓存未命中，解密并缓存
let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
    &state.encryptor,
    &encrypted,
).await?;

redis_conn.set_ex(&cache_key, &api_key, 3600).await?;
```

### 2. 批量操作

使用数据库的批量操作减少往返次数：

```rust
// 批量查询加密的卡密
let cards: Vec<(Uuid, String)> = sqlx::query_as(
    "SELECT id, code_encrypted FROM cards WHERE app_id = $1"
)
.bind(app_id)
.fetch_all(&state.pool)
.await?;

// 批量解密
let decrypted: Vec<(Uuid, String)> = futures::future::try_join_all(
    cards.iter().map(|(id, encrypted)| async move {
        let code = EncryptedFieldsOps::decrypt_card_code(
            &state.encryptor,
            encrypted,
        ).await?;
        Ok((*id, code))
    })
).await?;
```

### 3. 异步处理

对于大量数据的加密/解密，使用后台任务：

```rust
tokio::spawn(async move {
    if let Err(e) = encrypt_historical_data(&pool, &encryptor).await {
        tracing::error!("加密历史数据失败: {}", e);
    }
});
```

## 安全最佳实践

### 1. 访问控制

只有授权用户才能查看敏感字段：

```rust
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
    let encrypted = get_encrypted_api_key(&state.pool, merchant_id).await?;
    let api_key = EncryptedFieldsOps::decrypt_merchant_api_key(
        &state.encryptor,
        &encrypted,
    ).await?;

    Ok(Json(ApiKeyResponse { api_key }))
}
```

### 2. 审计日志

所有敏感操作都会自动记录到 `encrypted_fields_log` 表：

```sql
SELECT * FROM encrypted_fields_log
WHERE table_name = 'merchants' AND field_name = 'api_key'
ORDER BY encrypted_at DESC;
```

### 3. 日志脱敏

不要在日志中输出敏感数据：

```rust
// ✓ 正确
tracing::info!("加密 API Key: merchant_id={}", merchant_id);

// ✗ 错误
tracing::info!("加密 API Key: {}", api_key);
```

## 故障排查

### 问题：MASTER_KEY 格式错误

```
Error: MASTER_KEY 必须是 32 字节（64 个十六进制字符）
```

**解决**：

```bash
# 验证长度
echo -n "your_key" | wc -c
# 应该输出 64

# 重新生成
MASTER_KEY=$(openssl rand -hex 32)
```

### 问题：解密失败

```
Error: 解密失败: authentication tag mismatch
```

**原因**：
- 使用了错误的主密钥
- 加密数据被篡改
- 密钥版本不匹配

**解决**：
- 检查 MASTER_KEY 环境变量
- 验证数据库中的加密数据完整性
- 检查 key_id 是否正确

### 问题：性能下降

**优化**：
- 启用 Redis 缓存
- 使用连接池
- 批量操作
- 异步处理

## 测试

### 单元测试

```bash
cargo test --lib utils::kms
```

### 集成测试

```bash
# 启动测试数据库
docker-compose -f docker-compose.test.yml up

# 运行测试
cargo test --test '*' -- --test-threads=1
```

## 监控

### 关键指标

- 加密/解密操作耗时
- 缓存命中率
- 密钥轮换状态
- 审计日志大小

### 查询加密统计

```sql
-- 查看加密字段统计
SELECT 
    table_name,
    field_name,
    COUNT(*) as encrypted_count,
    MAX(encrypted_at) as last_encrypted
FROM encrypted_fields_log
GROUP BY table_name, field_name;

-- 查看密钥使用情况
SELECT 
    key_id,
    COUNT(*) as usage_count,
    MAX(encrypted_at) as last_used
FROM encrypted_fields_log
GROUP BY key_id
ORDER BY last_used DESC;
```

## 参考资源

- [ENCRYPTION_GUIDE.md](../ENCRYPTION_GUIDE.md) - 详细的加密指南
- [utils/encryption_examples.rs](src/utils/encryption_examples.rs) - 代码示例
- [AES-GCM 算法](https://en.wikipedia.org/wiki/Galois/Counter_Mode)
- [NIST 密钥管理](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-57pt1r5.pdf)

