-- KamiSM 完整数据库初始化脚本
-- 合并所有迁移步骤，确保幂等性

-- ============================================================================
-- 1. 创建基础表
-- ============================================================================

-- 平台管理员表
CREATE TABLE IF NOT EXISTS admins (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username VARCHAR(64) NOT NULL UNIQUE,
    password_hash VARCHAR(256) NOT NULL,
    email VARCHAR(128) NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 商户表（仅存储加密字段，明文字段已删除）
CREATE TABLE IF NOT EXISTS merchants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username VARCHAR(64) NOT NULL UNIQUE,
    password_hash VARCHAR(256) NOT NULL,
    api_key_encrypted TEXT NOT NULL,
    api_key_hash VARCHAR(64) NOT NULL UNIQUE,
    email_encrypted TEXT NOT NULL,
    email_hash VARCHAR(64) NOT NULL UNIQUE,
    status VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    plan VARCHAR(16) NOT NULL DEFAULT 'free' CHECK (plan IN ('free', 'pro')),
    plan_expires_at TIMESTAMPTZ,
    email_verified BOOLEAN NOT NULL DEFAULT FALSE,
    verify_token VARCHAR(128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 应用表
CREATE TABLE IF NOT EXISTS apps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    app_name VARCHAR(128) NOT NULL,
    description TEXT,
    status VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    downgraded BOOLEAN NOT NULL DEFAULT FALSE,
    admin_disabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 卡密表（仅存储加密字段，明文字段已删除）
CREATE TABLE IF NOT EXISTS cards (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    code_encrypted TEXT NOT NULL,
    code_hash VARCHAR(64) NOT NULL,
    duration_days INTEGER NOT NULL,
    max_devices INTEGER NOT NULL DEFAULT 1,
    status VARCHAR(16) NOT NULL DEFAULT 'unused' CHECK (status IN ('unused', 'active', 'expired', 'disabled')),
    downgraded BOOLEAN NOT NULL DEFAULT FALSE,
    admin_disabled BOOLEAN NOT NULL DEFAULT FALSE,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    activated_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ
);

-- 设备激活记录表（仅存储加密字段，明文字段已删除）
CREATE TABLE IF NOT EXISTS activations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    card_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    device_id_encrypted TEXT NOT NULL,
    device_id_hash VARCHAR(64) NOT NULL,
    device_name VARCHAR(128),
    ip_address VARCHAR(64),
    activated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_verified_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(card_id, device_id_hash)
);

-- 套餐配置表
CREATE TABLE IF NOT EXISTS plan_configs (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    plan        VARCHAR(16) NOT NULL UNIQUE CHECK (plan IN ('free', 'pro')),
    label       VARCHAR(32) NOT NULL,
    max_apps    INTEGER     NOT NULL DEFAULT 1,
    max_cards   INTEGER     NOT NULL DEFAULT 500,
    max_devices INTEGER     NOT NULL DEFAULT 3,
    max_gen_once INTEGER    NOT NULL DEFAULT 100,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 加密字段版本管理表
CREATE TABLE IF NOT EXISTS encryption_keys (
    id SERIAL PRIMARY KEY,
    key_id VARCHAR(64) NOT NULL UNIQUE,
    key_version INTEGER NOT NULL DEFAULT 1,
    algorithm VARCHAR(32) NOT NULL DEFAULT 'AES-256-GCM',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rotated_at TIMESTAMPTZ,
    status VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'rotated', 'retired'))
);

-- 敏感字段加密日志表
CREATE TABLE IF NOT EXISTS encrypted_fields_log (
    id BIGSERIAL PRIMARY KEY,
    table_name VARCHAR(64) NOT NULL,
    record_id UUID NOT NULL,
    field_name VARCHAR(64) NOT NULL,
    key_id VARCHAR(64) NOT NULL,
    encrypted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(table_name, record_id, field_name)
);

-- ============================================================================
-- 2. 创建索引
-- ============================================================================

-- 基础索引
CREATE INDEX IF NOT EXISTS idx_cards_app_id ON cards(app_id);
CREATE INDEX IF NOT EXISTS idx_cards_merchant_id ON cards(merchant_id);
CREATE INDEX IF NOT EXISTS idx_activations_card_id ON activations(card_id);
CREATE INDEX IF NOT EXISTS idx_apps_merchant_id ON apps(merchant_id);

-- 加密字段哈希索引（用于快速查询）
CREATE UNIQUE INDEX IF NOT EXISTS idx_merchants_api_key_hash ON merchants(api_key_hash);
CREATE UNIQUE INDEX IF NOT EXISTS idx_merchants_email_hash ON merchants(email_hash);
CREATE INDEX IF NOT EXISTS idx_cards_code_hash ON cards(code_hash);
CREATE INDEX IF NOT EXISTS idx_activations_device_id_hash ON activations(device_id_hash);

-- 加密日志索引
CREATE INDEX IF NOT EXISTS idx_encryption_keys_status ON encryption_keys(status);
CREATE INDEX IF NOT EXISTS idx_encrypted_fields_log_table ON encrypted_fields_log(table_name, record_id);

-- ============================================================================
-- 3. 初始化套餐配置
-- ============================================================================

INSERT INTO plan_configs (plan, label, max_apps, max_cards, max_devices, max_gen_once)
VALUES
    ('free', '免费版', 1,  500, 3,   100),
    ('pro',  '专业版', -1, -1,  100, 1000)
ON CONFLICT (plan) DO NOTHING;


-- ============================================================================
-- 站内信 / 公告系统
-- ============================================================================

-- 消息主表
-- type: 'notice' = 公告（全体可见，无需已读追踪）
--       'message' = 站内信（有收件人，追踪已读）
-- target_type: 'all' = 全体商户，'single' = 单个商户（仅 message 类型有效）
CREATE TABLE IF NOT EXISTS messages (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    type         VARCHAR(16) NOT NULL CHECK (type IN ('notice', 'message')),
    title        VARCHAR(256) NOT NULL,
    content      TEXT        NOT NULL,
    sender_id    UUID        NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    target_type  VARCHAR(16) NOT NULL DEFAULT 'all' CHECK (target_type IN ('all', 'single')),
    target_id    UUID        REFERENCES merchants(id) ON DELETE CASCADE,
    pinned       BOOLEAN     NOT NULL DEFAULT FALSE,
    expires_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 已读记录表（仅 type='message' 时使用）
-- 广播消息只存一行，各商户已读状态在此表追踪
CREATE TABLE IF NOT EXISTS message_reads (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id   UUID        NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    merchant_id  UUID        NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    read_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(message_id, merchant_id)
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_messages_type        ON messages(type);
CREATE INDEX IF NOT EXISTS idx_messages_sender_id   ON messages(sender_id);
CREATE INDEX IF NOT EXISTS idx_messages_target_id   ON messages(target_id);
CREATE INDEX IF NOT EXISTS idx_messages_pinned      ON messages(pinned) WHERE pinned = TRUE;
CREATE INDEX IF NOT EXISTS idx_messages_expires_at  ON messages(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_message_reads_merchant ON message_reads(merchant_id);
CREATE INDEX IF NOT EXISTS idx_message_reads_message  ON message_reads(message_id);

