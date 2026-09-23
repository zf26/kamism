-- ============================================================================
-- Token 吊销：为 merchants / admins 增加 token_version 计数器
-- ============================================================================
--
-- 背景：此前签发的 access token（2 小时）与 refresh token（7 天）都是**纯无状态 JWT**，
-- 签名正确即有效。后果有两层：
--
--   1. 改密码 / 邮箱重置密码后，**旧 token 依然有效**。攻击者拿到 refresh token 后，
--      即使受害者改了密码，仍能续期。
--   2. 更严重的是「滚动续期」：/auth/refresh 每次都会签发**新的** refresh token
--      （auth.rs 的 refresh_token handler），所以攻击者只要每 7 天用一次，
--      理论上可以**无限期**维持访问 —— 不是「最多 7 天」。
--   3. 管理员禁用商户（admin.rs 的 update_merchant_status）后，该商户的
--      access token 仍能用最多 2 小时 —— 封号封不住。
--
-- 方案：给每个用户一个单调递增的 token_version 计数器，
--   签发时把当时的值写进 claims.ver，校验时比对。
--   任何需要「踢掉所有已签发令牌」的动作只需 `token_version = token_version + 1`。
--
-- 为什么不用黑名单表：
--   黑名单要存「每一条签发过的 token」（或至少每一条需要在有效期内吊销的），
--   表会随登录量无限增长，还得配套清理任务。token_version 一行字段解决全部场景 ——
--   代价是**无法只踢掉某一个设备**（要么全踢、要么不踢）。对这个业务够用。
--
-- ⚠️ 默认值必须是 0（而不是从某个时间戳开始）：
--   存量用户在升级瞬间的旧 token 里**没有 ver 字段**，反序列化会拿默认值 0；
--   库里也是 0 → 两边相等 → **平滑过渡，不会把所有在线用户踢下线**。
--   这正是选 0 而不是 NOW() 的原因。见 utils/jwt.rs 里 Claims.ver 的
--   `#[serde(default)]` —— 两者必须保持一致，改一个就要改另一个。

ALTER TABLE merchants ADD COLUMN IF NOT EXISTS token_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE admins    ADD COLUMN IF NOT EXISTS token_version INTEGER NOT NULL DEFAULT 0;

COMMENT ON COLUMN merchants.token_version IS
  '令牌版本号：签发 JWT 时写入 claims.ver，校验时比对。改密码/重置/禁用等动作 +1 即可吊销该用户全部已签发令牌。0 = 从未吊销。';
COMMENT ON COLUMN admins.token_version IS
  '同上，用于 admins 表。';
