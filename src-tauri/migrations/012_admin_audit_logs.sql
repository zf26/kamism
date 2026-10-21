-- 管理员操作审计日志。
--
-- 背景：管理后台能做的事包括「给任意商户开专业版 / 改到期时间 / 禁用 / 删除商户 /
-- 改支付渠道密钥 / 改 OAuth 密钥」。改造前这些动作**全仓库 0 条留痕** ——
-- 内部滥用查不出、误操作无法还原、商户来争议时拿不出证据。
--
-- 设计取舍：
--
-- 1. `admin_id` **不加外键**。管理员账号被删（或换人接手）之后，他做过的操作
--    仍然必须留在库里 —— 加 `REFERENCES admins(id) ON DELETE CASCADE` 会让
--    "删除管理员"顺手擦掉全部历史，正好与审计的目的相反。
--    顺带存一份 `admin_email`：取证时不必 join 一张可能已经没有这一行的表。
--
-- 2. `ip` 用 TEXT 而不是 INET。写入方是应用层（已按 TRUSTED_PROXIES 策略
--    解析出真实客户端 IP），存文本可以直接 `LIKE '203.0.113.%'` 做排查；
--    用 INET 会引入额外的类型转换失败面，而这里不做任何网络运算。
--
-- 3. 只记**写操作**（非 GET）。读操作量大且价值低，全记会让这张表很快变成
--    新的性能问题，也会把真正要看的几行埋掉。
--
-- 4. `detail` 只存**白名单字段**（plan / days / status / enabled …）。
--    请求体里可能有 client_secret、支付私钥，**绝不能**整包落库 ——
--    审计日志本身不能变成新的凭据泄露源。

CREATE TABLE IF NOT EXISTS admin_audit_logs (
    id          BIGSERIAL PRIMARY KEY,
    admin_id    UUID,
    admin_email VARCHAR(128),
    method      VARCHAR(8)   NOT NULL,
    -- 路由模板（如 /admin/merchants/:id/plan），便于按"做了什么动作"聚合
    route       VARCHAR(200) NOT NULL,
    -- 实际路径（含具体 id），便于按"动了哪个对象"检索
    path        VARCHAR(500) NOT NULL,
    status      SMALLINT     NOT NULL,
    detail      JSONB,
    ip          TEXT,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- 后台列表默认按时间倒序翻页
CREATE INDEX IF NOT EXISTS idx_admin_audit_logs_created_at
    ON admin_audit_logs (created_at DESC);

-- 「查某个管理员做过什么」
CREATE INDEX IF NOT EXISTS idx_admin_audit_logs_admin
    ON admin_audit_logs (admin_id, created_at DESC);

COMMENT ON TABLE admin_audit_logs IS
  '管理员写操作审计：谁、何时、从哪个 IP、对哪个对象做了什么、结果如何。detail 仅含白名单字段，不含任何密钥。';
