-- KamiSM 完整数据库初始化脚本
-- 合并所有迁移步骤，确保幂等性
-- 可直接执行，无需顺序依赖

BEGIN;

-- ============================================================================
-- 1. 基础表
-- ============================================================================

CREATE TABLE IF NOT EXISTS admins (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username VARCHAR(64) NOT NULL UNIQUE,
    password_hash VARCHAR(256) NOT NULL,
    email VARCHAR(128) NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

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
    created_by_admin BOOLEAN NOT NULL DEFAULT FALSE,  -- true=管理员创建，false=自助注册
    invited_by UUID,  -- 代理注册时记录上级关系ID（外键在 agent_relations 创建后添加）
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

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

CREATE TABLE IF NOT EXISTS encryption_keys (
    id SERIAL PRIMARY KEY,
    key_id VARCHAR(64) NOT NULL UNIQUE,
    key_version INTEGER NOT NULL DEFAULT 1,
    algorithm VARCHAR(32) NOT NULL DEFAULT 'AES-256-GCM',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rotated_at TIMESTAMPTZ,
    status VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'rotated', 'retired'))
);

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
-- 2. 站内信 / 公告系统
-- ============================================================================

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

CREATE TABLE IF NOT EXISTS message_reads (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id   UUID        NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    merchant_id  UUID        NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    read_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(message_id, merchant_id)
);

-- ============================================================================
-- 3. Webhook 配置表
-- ============================================================================

CREATE TABLE IF NOT EXISTS app_webhooks (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id      UUID        NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    merchant_id UUID        NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    url         TEXT        NOT NULL,
    secret      VARCHAR(64) NOT NULL,
    enabled     BOOLEAN     NOT NULL DEFAULT TRUE,
    events      TEXT[]      NOT NULL DEFAULT ARRAY['activate', 'verify'],
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(app_id)
);

-- ============================================================================
-- 4. 风控相关表
-- ============================================================================

CREATE TABLE IF NOT EXISTS ip_blacklist (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id UUID REFERENCES merchants(id) ON DELETE CASCADE,
    ip          VARCHAR(64) NOT NULL,
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ip_blacklist
    ON ip_blacklist (COALESCE(merchant_id::text, 'global'), ip);

CREATE TABLE IF NOT EXISTS device_blacklist (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id     UUID REFERENCES merchants(id) ON DELETE CASCADE,
    device_id_hash  VARCHAR(64) NOT NULL,
    device_hint     VARCHAR(64),
    reason          TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_device_blacklist
    ON device_blacklist (COALESCE(merchant_id::text, 'global'), device_id_hash);

CREATE TABLE IF NOT EXISTS activation_alerts (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id  UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    alert_type   VARCHAR(32) NOT NULL,
    card_id      UUID REFERENCES cards(id) ON DELETE SET NULL,
    device_hint  VARCHAR(64),
    ip_address   VARCHAR(64),
    detail       TEXT,
    is_read      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 5. 代理体系
-- ============================================================================

CREATE TABLE IF NOT EXISTS agent_relations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    parent_id       UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    agent_id        UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    quota_total     INTEGER NOT NULL DEFAULT 0,
    quota_used      INTEGER NOT NULL DEFAULT 0,
    commission_rate INTEGER NOT NULL DEFAULT 0 CHECK (commission_rate >= 0 AND commission_rate <= 100),
    status          VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    invite_code     VARCHAR(32) NOT NULL UNIQUE,
    note            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(parent_id, agent_id)
);

-- merchants.invited_by 外键延迟到 agent_relations 创建后添加（避免循环依赖）
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'fk_merchants_invited_by'
    ) THEN
        ALTER TABLE merchants ADD CONSTRAINT fk_merchants_invited_by
            FOREIGN KEY (invited_by) REFERENCES agent_relations(id) ON DELETE SET NULL;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS agent_quota_logs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    relation_id UUID NOT NULL REFERENCES agent_relations(id) ON DELETE CASCADE,
    parent_id   UUID NOT NULL,
    agent_id    UUID NOT NULL,
    delta       INTEGER NOT NULL,
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS agent_commission_logs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    relation_id     UUID NOT NULL REFERENCES agent_relations(id) ON DELETE CASCADE,
    agent_id        UUID NOT NULL,
    parent_id       UUID NOT NULL,
    card_id         UUID REFERENCES cards(id) ON DELETE SET NULL,
    activation_id   UUID,
    commission_rate INTEGER NOT NULL,
    units           INTEGER NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ============================================================================
-- 6. 支付订单表
-- ============================================================================

CREATE TABLE IF NOT EXISTS payments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id     UUID        NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    order_id        VARCHAR(64) NOT NULL UNIQUE,
    pay_channel     VARCHAR(16) NOT NULL DEFAULT 'xorpay'
                            CHECK (pay_channel IN ('xorpay', 'mbdpay')),
    xorpay_aoid     VARCHAR(64),
    mbdpay_charge_id VARCHAR(64),
    pay_type        VARCHAR(16) NOT NULL CHECK (pay_type IN ('wechat', 'alipay')),
    amount          DECIMAL(10,2) NOT NULL,
    status          VARCHAR(16) NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'paid', 'expired', 'refunded')),
    plan            VARCHAR(16) NOT NULL,
    expires_days    INTEGER,
    pay_price       DECIMAL(10,2),
    pay_time        TIMESTAMPTZ,
    notify_data     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ============================================================================
-- 7. 索引
-- ============================================================================

CREATE INDEX IF NOT EXISTS idx_cards_app_id          ON cards(app_id);
CREATE INDEX IF NOT EXISTS idx_cards_merchant_id     ON cards(merchant_id);
CREATE INDEX IF NOT EXISTS idx_activations_card_id    ON activations(card_id);
CREATE INDEX IF NOT EXISTS idx_apps_merchant_id      ON apps(merchant_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_merchants_api_key_hash ON merchants(api_key_hash);
CREATE UNIQUE INDEX IF NOT EXISTS idx_merchants_email_hash    ON merchants(email_hash);
CREATE INDEX IF NOT EXISTS idx_cards_code_hash        ON cards(code_hash);
CREATE INDEX IF NOT EXISTS idx_activations_device_id_hash ON activations(device_id_hash);

CREATE INDEX IF NOT EXISTS idx_encryption_keys_status ON encryption_keys(status);
CREATE INDEX IF NOT EXISTS idx_encrypted_fields_log_table ON encrypted_fields_log(table_name, record_id);

CREATE INDEX IF NOT EXISTS idx_messages_type        ON messages(type);
CREATE INDEX IF NOT EXISTS idx_messages_sender_id   ON messages(sender_id);
CREATE INDEX IF NOT EXISTS idx_messages_target_id   ON messages(target_id);
CREATE INDEX IF NOT EXISTS idx_messages_pinned      ON messages(pinned) WHERE pinned = TRUE;
CREATE INDEX IF NOT EXISTS idx_messages_expires_at  ON messages(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_messages_pinned_created ON messages(pinned DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_message_reads_merchant ON message_reads(merchant_id);
CREATE INDEX IF NOT EXISTS idx_message_reads_message  ON message_reads(message_id);

CREATE INDEX IF NOT EXISTS idx_app_webhooks_merchant ON app_webhooks(merchant_id);
CREATE INDEX IF NOT EXISTS idx_app_webhooks_app     ON app_webhooks(app_id);

CREATE INDEX IF NOT EXISTS idx_alerts_merchant ON activation_alerts(merchant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_unread   ON activation_alerts(merchant_id, is_read) WHERE is_read = FALSE;

CREATE INDEX IF NOT EXISTS idx_agent_relations_parent ON agent_relations(parent_id);
CREATE INDEX IF NOT EXISTS idx_agent_relations_agent  ON agent_relations(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_invite_code      ON agent_relations(invite_code);
CREATE INDEX IF NOT EXISTS idx_quota_logs_relation    ON agent_quota_logs(relation_id);
CREATE INDEX IF NOT EXISTS idx_commission_agent       ON agent_commission_logs(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_commission_parent      ON agent_commission_logs(parent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_commission_relation    ON agent_commission_logs(relation_id);

CREATE INDEX IF NOT EXISTS idx_payments_merchant_id ON payments(merchant_id);
CREATE INDEX IF NOT EXISTS idx_payments_order_id     ON payments(order_id);
CREATE INDEX IF NOT EXISTS idx_payments_xorpay_aoid  ON payments(xorpay_aoid);
CREATE INDEX IF NOT EXISTS idx_payments_status       ON payments(status);
CREATE INDEX IF NOT EXISTS idx_payments_pay_channel  ON payments(pay_channel);
CREATE INDEX IF NOT EXISTS idx_payments_mbdpay_charge_id ON payments(mbdpay_charge_id) WHERE mbdpay_charge_id IS NOT NULL;

-- 性能优化：复合索引
CREATE UNIQUE INDEX IF NOT EXISTS idx_cards_code_hash_merchant_app ON cards(code_hash, merchant_id, app_id);
CREATE INDEX IF NOT EXISTS idx_cards_merchant_created  ON cards(merchant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_cards_merchant_status    ON cards(merchant_id, status);
CREATE INDEX IF NOT EXISTS idx_cards_status_expires     ON cards(status, expires_at) WHERE status = 'active' AND expires_at IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_activations_card_device_hash ON activations(card_id, device_id_hash);
CREATE INDEX IF NOT EXISTS idx_activations_activated_at  ON activations(activated_at DESC);
CREATE INDEX IF NOT EXISTS idx_activations_last_verified ON activations(last_verified_at DESC);

CREATE INDEX IF NOT EXISTS idx_merchants_plan_expires   ON merchants(plan, plan_expires_at) WHERE plan = 'pro' AND plan_expires_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_apps_id_merchant_status  ON apps(id, merchant_id, status);
CREATE INDEX IF NOT EXISTS idx_apps_merchant_downgraded ON apps(merchant_id, downgraded) WHERE downgraded = TRUE;

-- ============================================================================
-- 8. 初始化数据
-- ============================================================================

INSERT INTO plan_configs (plan, label, max_apps, max_cards, max_devices, max_gen_once)
VALUES
    ('free', '免费版', 1,  500, 3,   100),
    ('pro',  '专业版', -1, -1,  100, 1000)
ON CONFLICT (plan) DO NOTHING;

-- ============================================================================
-- 老数据库升级：补充 pay_channel 和 mbdpay_charge_id 字段
-- ============================================================================
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'payments' AND column_name = 'pay_channel') THEN
        ALTER TABLE payments ADD COLUMN pay_channel VARCHAR(16) NOT NULL DEFAULT 'xorpay'
            CHECK (pay_channel IN ('xorpay', 'mbdpay'));
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'payments' AND column_name = 'mbdpay_charge_id') THEN
        ALTER TABLE payments ADD COLUMN mbdpay_charge_id VARCHAR(64);
    END IF;
END $$;

COMMIT;
