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

