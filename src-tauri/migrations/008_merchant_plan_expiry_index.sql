-- ============================================================================
-- 为商户套餐到期扫描添加索引（scan_and_enqueue 查询）
-- ============================================================================
CREATE INDEX IF NOT EXISTS idx_merchants_plan_expiry
    ON merchants(plan_expires_at)
    WHERE plan = 'pro' AND plan_expires_at IS NOT NULL;
