-- 商户套餐字段
ALTER TABLE merchants ADD COLUMN IF NOT EXISTS plan VARCHAR(16) NOT NULL DEFAULT 'free' CHECK (plan IN ('free', 'pro'));

