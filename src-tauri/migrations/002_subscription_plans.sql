-- 订阅套餐管理表
BEGIN;

CREATE TABLE IF NOT EXISTS subscription_plans (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    plan            VARCHAR(32) NOT NULL,          -- 套餐标识，如 "pro_monthly", "pro_yearly", "pro_lifetime"
    name            VARCHAR(64) NOT NULL,          -- 前端展示名称，如 "30 天续费"
    days            INTEGER,                     -- 天数，NULL 表示永久
    price           DECIMAL(10,2) NOT NULL,        -- 价格（元）
    original_price  DECIMAL(10,2),                 -- 划线价（可选，用于显示优惠）
    badge           VARCHAR(32),                   -- 角标文字，如 "省 9 元"
    highlight       BOOLEAN     NOT NULL DEFAULT FALSE,  -- 是否高亮推荐
    sort_order      INTEGER     NOT NULL DEFAULT 0,      -- 排序（小的排前面）
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,   -- 是否启用
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(plan)
);

-- 初始化默认套餐数据（plan 只能是 free 或 pro，对应 plan_configs.plan）
INSERT INTO subscription_plans (plan, name, days, price, original_price, badge, highlight, sort_order)
VALUES
    ('pro', '30 天续费',   30,   30.00,  NULL, NULL,         FALSE, 1),
    ('pro', '90 天续费',   90,   90.00,  NULL, '省 9 元',    FALSE, 2),
    ('pro', '180 天续费',  180,  180.00, NULL, '省 18 元',   FALSE, 3),
    ('pro', '永久专业版',  NULL, 365.00, NULL, '最划算',     TRUE,  4)
ON CONFLICT DO NOTHING;

COMMIT;
