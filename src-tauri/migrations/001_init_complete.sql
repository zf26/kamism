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

-- ============================================================================
-- Webhook 配置表
-- ============================================================================

-- 每个应用可配置一个 Webhook URL，激活/验证成功时推送事件
CREATE TABLE IF NOT EXISTS app_webhooks (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id      UUID        NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    merchant_id UUID        NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    url         TEXT        NOT NULL,
    secret      VARCHAR(64) NOT NULL,          -- HMAC-SHA256 签名密钥
    enabled     BOOLEAN     NOT NULL DEFAULT TRUE,
    events      TEXT[]      NOT NULL DEFAULT ARRAY['activate', 'verify'],
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(app_id)
);

CREATE INDEX IF NOT EXISTS idx_app_webhooks_merchant ON app_webhooks(merchant_id);
CREATE INDEX IF NOT EXISTS idx_app_webhooks_app     ON app_webhooks(app_id);

-- ============================================================================
-- 002_perf_indexes.sql  性能优化：补充复合索引 / 覆盖索引
-- 全部幂等（IF NOT EXISTS / DO NOTHING），可重复执行
-- ============================================================================

-- ── cards 表 ──────────────────────────────────────────────────────────────

-- 公开 API 核心热路径：code_hash + merchant_id + app_id 三列联合查询
-- 替代原来分散的单列索引，减少 Index Scan 次数
CREATE UNIQUE INDEX IF NOT EXISTS idx_cards_code_hash_merchant_app
    ON cards(code_hash, merchant_id, app_id);

-- 商户分页列表：merchant_id + created_at DESC，覆盖排序列避免 filesort
CREATE INDEX IF NOT EXISTS idx_cards_merchant_created
    ON cards(merchant_id, created_at DESC);

-- 状态过滤（分页 + 统计）
CREATE INDEX IF NOT EXISTS idx_cards_merchant_status
    ON cards(merchant_id, status);

-- 到期扫描（定时器扫描 active 且已过期的卡密）
CREATE INDEX IF NOT EXISTS idx_cards_status_expires
    ON cards(status, expires_at)
    WHERE status = 'active' AND expires_at IS NOT NULL;

-- ── activations 表 ──────────────────────────────────────────────────────

-- verify/activate 热路径：card_id + device_id_hash（已有 UNIQUE，确保存在）
CREATE UNIQUE INDEX IF NOT EXISTS idx_activations_card_device_hash
    ON activations(card_id, device_id_hash);

-- 商户激活记录列表（JOIN cards 后按时间排序）
CREATE INDEX IF NOT EXISTS idx_activations_activated_at
    ON activations(activated_at DESC);

-- 设备最后验证时间（用于清理过期设备的后台任务）
CREATE INDEX IF NOT EXISTS idx_activations_last_verified
    ON activations(last_verified_at DESC);

-- ── merchants 表 ─────────────────────────────────────────────────────────

-- 定时降级扫描：plan + plan_expires_at（仅扫描 pro 且已过期的行）
CREATE INDEX IF NOT EXISTS idx_merchants_plan_expires
    ON merchants(plan, plan_expires_at)
    WHERE plan = 'pro' AND plan_expires_at IS NOT NULL;

-- ── apps 表 ─────────────────────────────────────────────────────────────

-- 公开 API 应用鉴权：id + merchant_id + status 联合
CREATE INDEX IF NOT EXISTS idx_apps_id_merchant_status
    ON apps(id, merchant_id, status);

-- 商户降级恢复查询
CREATE INDEX IF NOT EXISTS idx_apps_merchant_downgraded
    ON apps(merchant_id, downgraded)
    WHERE downgraded = TRUE;

-- ── messages 表 ──────────────────────────────────────────────────────────

-- 公告列表：pinned DESC + created_at DESC（常用排序）
CREATE INDEX IF NOT EXISTS idx_messages_pinned_created
    ON messages(pinned DESC, created_at DESC);

-- 到期公告过滤（只在有 expires_at 的行上）
-- 已在 001 中创建：idx_messages_expires_at，此处无需重复

-- ============================================================================
-- 003_risk_control.sql  风控相关表结构
-- ============================================================================

-- IP 黑名单
CREATE TABLE IF NOT EXISTS ip_blacklist (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id UUID REFERENCES merchants(id) ON DELETE CASCADE,  -- NULL = 全局（管理员设置）
    ip          VARCHAR(64) NOT NULL,
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ip_blacklist
    ON ip_blacklist (COALESCE(merchant_id::text, 'global'), ip);

-- 设备黑名单（存储 device_id_hash，无需解密即可比对）
CREATE TABLE IF NOT EXISTS device_blacklist (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id UUID REFERENCES merchants(id) ON DELETE CASCADE,  -- NULL = 全局
    device_id_hash VARCHAR(64) NOT NULL,
    device_hint VARCHAR(64),   -- 展示用的脱敏标识，如 "ABCD****"
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_device_blacklist
    ON device_blacklist (COALESCE(merchant_id::text, 'global'), device_id_hash);

-- 异常激活告警记录
CREATE TABLE IF NOT EXISTS activation_alerts (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id  UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    alert_type   VARCHAR(32) NOT NULL,  -- 'ip_abuse' | 'device_multi_card' | 'card_geo_jump'
    card_id      UUID REFERENCES cards(id) ON DELETE SET NULL,
    device_hint  VARCHAR(64),
    ip_address   VARCHAR(64),
    detail       TEXT,
    is_read      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_alerts_merchant ON activation_alerts(merchant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_unread   ON activation_alerts(merchant_id, is_read) WHERE is_read = FALSE;

-- ============================================================================
-- 004_agent_system.sql  多级代理体系
-- ============================================================================

-- 代理关系表：上级商户 → 下级代理（最多 2 级）
CREATE TABLE IF NOT EXISTS agent_relations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    parent_id       UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,  -- 上级商户
    agent_id        UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,  -- 下级代理
    -- 配额：上级划拨给代理的最大卡密生成数量（-1 = 不限）
    quota_total     INTEGER NOT NULL DEFAULT 0,
    quota_used      INTEGER NOT NULL DEFAULT 0,
    -- 分润比例（0-100 整数，表示百分比，如 20 = 20%）
    commission_rate INTEGER NOT NULL DEFAULT 0 CHECK (commission_rate >= 0 AND commission_rate <= 100),
    status          VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    invite_code     VARCHAR(32) NOT NULL UNIQUE,  -- 邀请码，代理注册时填写
    note            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(parent_id, agent_id)
);
CREATE INDEX IF NOT EXISTS idx_agent_relations_parent ON agent_relations(parent_id);
CREATE INDEX IF NOT EXISTS idx_agent_relations_agent  ON agent_relations(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_invite_code      ON agent_relations(invite_code);

-- 配额调整日志
CREATE TABLE IF NOT EXISTS agent_quota_logs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    relation_id UUID NOT NULL REFERENCES agent_relations(id) ON DELETE CASCADE,
    parent_id   UUID NOT NULL,
    agent_id    UUID NOT NULL,
    delta       INTEGER NOT NULL,   -- 正数增加，负数回收
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_quota_logs_relation ON agent_quota_logs(relation_id);

-- 分润记录（每次激活时异步写入）
CREATE TABLE IF NOT EXISTS agent_commission_logs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    relation_id   UUID NOT NULL REFERENCES agent_relations(id) ON DELETE CASCADE,
    agent_id      UUID NOT NULL,
    parent_id     UUID NOT NULL,
    card_id       UUID REFERENCES cards(id) ON DELETE SET NULL,
    activation_id UUID,
    commission_rate INTEGER NOT NULL,
    -- 仅统计用，不涉及真实金额；单位：张（1 张卡密激活 = 1 个计量单位）
    units         INTEGER NOT NULL DEFAULT 1,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_commission_agent    ON agent_commission_logs(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_commission_parent   ON agent_commission_logs(parent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_commission_relation ON agent_commission_logs(relation_id);

-- merchants 表添加 invited_by 字段（代理注册时记录上级关系 ID）
ALTER TABLE merchants ADD COLUMN IF NOT EXISTS invited_by UUID REFERENCES agent_relations(id) ON DELETE SET NULL;

