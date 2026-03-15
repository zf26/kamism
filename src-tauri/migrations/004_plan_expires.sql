-- 商户专业版到期时间（NULL 表示永久有效）
ALTER TABLE merchants ADD COLUMN IF NOT EXISTS plan_expires_at TIMESTAMPTZ;

