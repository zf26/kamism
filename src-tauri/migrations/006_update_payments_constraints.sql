-- ============================================================================
-- 更新 payments 表约束：支持 alipay 渠道和 cancelled 状态
-- ============================================================================

-- 1. 扩展 pay_channel 约束，加入 alipay
ALTER TABLE payments DROP CONSTRAINT IF EXISTS payments_pay_channel_check;
ALTER TABLE payments ADD CONSTRAINT payments_pay_channel_check
    CHECK (pay_channel IN ('xorpay', 'mbdpay', 'alipay'));

-- 2. 扩展 status 约束，加入 cancelled
ALTER TABLE payments DROP CONSTRAINT IF EXISTS payments_status_check;
ALTER TABLE payments ADD CONSTRAINT payments_status_check
    CHECK (status IN ('pending', 'paid', 'expired', 'refunded', 'cancelled'));
