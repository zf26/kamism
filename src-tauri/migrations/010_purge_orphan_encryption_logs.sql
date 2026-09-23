-- ============================================================================
-- 加密日志卫生：清掉存量孤儿 + 让孤儿无法再产生
-- ============================================================================
--
-- 背景：`encrypted_fields_log` 记的是「某张表的某一行、某个字段，是用哪个 key_id
-- 加密的」。它**没有外键** —— `(table_name, record_id)` 是多态指向（同一个
-- record_id 列分别指向 cards / activations / merchants），数据库层面建不出 FK。
-- 所以业务行被删时，数据库既不会连带清理它，也不会报错，只会安静地积累垃圾。
--
-- 实测（写这条迁移时的库状态，可用 psql 复现）：
--     merchants / apps / cards / activations = 全部 0 行
--     encrypted_fields_log                   = 139 行
--     其中「record_id 在对应表里已不存在」的 = 139 行（卡密 46 + 激活 93）
--    → 整张表里**没有一条有效数据**，全是孤儿。
--
-- 孤儿从哪来（两个方向，都已在本批修复）：
--   1. 写入顺序：先写日志、后插业务行，且两者不在同一个事务里
--      （`auth.rs` 注册、`oauth.rs` 三方登录）—— 业务 INSERT 失败（比如用户名
--      重复这种最常见的错误）→ 日志已经提交 → 孤儿。
--   2. 删除：业务行被删（含 `ON DELETE CASCADE` 级联）→ 日志留着。
--      本迁移的触发器就是修这个方向。
--
-- 为什么用触发器，而不是在各个删除入口补一行 DELETE：
--   删除入口不止一处，而且最要命的是**级联删除** —— 删 app 或删商户会由数据库
--   连带删掉 cards / activations，这是数据库自己做的，应用层根本拿不到那些 id，
--   想补清理也无从下手。在应用层逐个入口补，漏一个就开始积累，而且漏了没有任何提示。
--   触发器挂在表上，覆盖全部删除路径（含级联），一次写对、永久生效。
--
-- ⚠️ 两个方向缺一不可：触发器只管「之后」的删除，已经躺在表里的存量孤儿
--    必须由下面的 DELETE 清掉。以后如果再出现写入顺序类的新入口，
--    触发器是拦不住的 —— 那种情况只能靠「写入必须和业务行同一个事务」这条纪律
--    （代码侧已把「事务外写日志」的公共方法全部删除，见
--     `db/encrypted_fields.rs` 的模块说明）。

-- ── 1. 清理存量孤儿 ────────────────────────────────────────────────────────
DELETE FROM encrypted_fields_log l
WHERE (l.table_name = 'cards'
       AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.id = l.record_id))
   OR (l.table_name = 'activations'
       AND NOT EXISTS (SELECT 1 FROM activations a WHERE a.id = l.record_id))
   OR (l.table_name = 'merchants'
       AND NOT EXISTS (SELECT 1 FROM merchants m WHERE m.id = l.record_id));

-- ── 2. 触发器：业务行一删，对应日志立刻跟着删 ─────────────────────────────
--
-- 用 TG_ARGV[0] 传入表名，一个函数服务三张表 —— 避免三份几乎相同的
-- plpgsql 各自漂移。注意 plpgsql 的数组下标从 0 开始（不是 1）。
CREATE OR REPLACE FUNCTION purge_encrypted_fields_log() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM encrypted_fields_log
     WHERE table_name = TG_ARGV[0]
       AND record_id  = OLD.id;
    RETURN OLD;
END;
$$;

COMMENT ON FUNCTION purge_encrypted_fields_log() IS
  'AFTER DELETE 触发器函数：删除业务行时连带清掉 encrypted_fields_log 里对应的记录。参数 TG_ARGV[0] = 表名（cards/activations/merchants）。这张日志表没有外键，删除不会自动清理，靠本触发器兜住。';

-- 每张有日志的表都要装。`DROP TRIGGER IF EXISTS` 是为了让本迁移可重复执行。
DROP TRIGGER IF EXISTS trg_purge_log_cards ON cards;
CREATE TRIGGER trg_purge_log_cards
    AFTER DELETE ON cards
    FOR EACH ROW EXECUTE FUNCTION purge_encrypted_fields_log('cards');

DROP TRIGGER IF EXISTS trg_purge_log_activations ON activations;
CREATE TRIGGER trg_purge_log_activations
    AFTER DELETE ON activations
    FOR EACH ROW EXECUTE FUNCTION purge_encrypted_fields_log('activations');

DROP TRIGGER IF EXISTS trg_purge_log_merchants ON merchants;
CREATE TRIGGER trg_purge_log_merchants
    AFTER DELETE ON merchants
    FOR EACH ROW EXECUTE FUNCTION purge_encrypted_fields_log('merchants');
