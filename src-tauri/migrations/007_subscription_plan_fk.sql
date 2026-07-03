-- 限制 subscription_plans.plan 只能是 plan_configs.plan（free/pro）
BEGIN;

-- 将现有数据的 plan 值归一化到 free/pro
UPDATE subscription_plans SET plan = 'pro' WHERE plan NOT IN ('free', 'pro');

-- 删除旧的 UNIQUE(plan) 约束
ALTER TABLE subscription_plans DROP CONSTRAINT IF EXISTS subscription_plans_plan_key;

-- 添加 CHECK 约束，plan 只能是 free 或 pro
ALTER TABLE subscription_plans DROP CONSTRAINT IF EXISTS chk_subscription_plans_plan;
ALTER TABLE subscription_plans
    ADD CONSTRAINT chk_subscription_plans_plan
    CHECK (plan IN ('free', 'pro'));

-- 添加外键约束（引用 plan_configs.plan）
ALTER TABLE subscription_plans DROP CONSTRAINT IF EXISTS fk_subscription_plans_plan;
ALTER TABLE subscription_plans
    ADD CONSTRAINT fk_subscription_plans_plan
    FOREIGN KEY (plan) REFERENCES plan_configs(plan);

-- 添加复合唯一约束：同一种 plan 的不同天数组合不能重复
ALTER TABLE subscription_plans DROP CONSTRAINT IF EXISTS uq_subscription_plans_plan_days;
ALTER TABLE subscription_plans
    ADD CONSTRAINT uq_subscription_plans_plan_days
    UNIQUE (plan, days);

COMMIT;
