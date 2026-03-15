-- 套餐配置表（每个 plan 一行，管理员可动态修改限制）
CREATE TABLE IF NOT EXISTS plan_configs (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    plan        VARCHAR(16) NOT NULL UNIQUE CHECK (plan IN ('free', 'pro')),
    label       VARCHAR(32) NOT NULL,           -- 显示名称，如 "免费版"
    max_apps    INTEGER     NOT NULL DEFAULT 1,  -- -1 表示无限
    max_cards   INTEGER     NOT NULL DEFAULT 500,
    max_devices INTEGER     NOT NULL DEFAULT 3,  -- 单张卡密最多设备数
    max_gen_once INTEGER    NOT NULL DEFAULT 100, -- 单次最多生成卡密数
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 初始数据
INSERT INTO plan_configs (plan, label, max_apps, max_cards, max_devices, max_gen_once)
VALUES
    ('free', '免费版', 1,  500, 3,   100),
    ('pro',  '专业版', -1, -1,  100, 1000)
ON CONFLICT (plan) DO NOTHING;

