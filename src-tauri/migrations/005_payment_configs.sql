-- 支付渠道配置表 - 支持多通道动态配置
BEGIN;

CREATE TABLE IF NOT EXISTS payment_configs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    channel         VARCHAR(32) NOT NULL UNIQUE
                            CHECK (channel IN ('alipay', 'xorpay', 'mbdpay')),
    name            VARCHAR(64) NOT NULL,
    enabled         BOOLEAN     NOT NULL DEFAULT FALSE,
    -- XorPay
    xorpay_aid      TEXT,
    xorpay_app_key  TEXT,
    xorpay_notify_url TEXT,
    -- MbdPay
    mbdpay_app_id   TEXT,
    mbdpay_app_key  TEXT,
    mbdpay_notify_url TEXT,
    -- Alipay
    alipay_app_id   TEXT,
    alipay_private_key TEXT,
    alipay_public_key TEXT,
    alipay_notify_url  TEXT,
    alipay_gateway     TEXT,
    alipay_return_url  TEXT,
    -- 通用
    extra_config    JSONB       DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_payment_configs_channel ON payment_configs(channel);
CREATE INDEX IF NOT EXISTS idx_payment_configs_enabled  ON payment_configs(enabled) WHERE enabled = TRUE;

-- 初始化支付宝配置（禁用状态）
INSERT INTO payment_configs (channel, name, enabled, alipay_notify_url, alipay_gateway, alipay_return_url)
VALUES (
    'alipay',
    '支付宝电脑网站支付',
    FALSE,
    'https://yourdomain/api/pay/notify',
    'https://openapi.alipay.com/gateway.do',
    'https://yourdomain/dashboard'
) ON CONFLICT (channel) DO NOTHING;

-- 初始化 XorPay 配置（禁用状态）
INSERT INTO payment_configs (channel, name, enabled, xorpay_notify_url)
VALUES (
    'xorpay',
    'XorPay',
    FALSE,
    'https://yourdomain/api/pay/notify'
) ON CONFLICT (channel) DO NOTHING;

-- 初始化 MbdPay 配置（禁用状态）
INSERT INTO payment_configs (channel, name, enabled, mbdpay_notify_url)
VALUES (
    'mbdpay',
    'MbdPay（面包多）',
    FALSE,
    'https://yourdomain/api/pay/notify'
) ON CONFLICT (channel) DO NOTHING;

COMMIT;
