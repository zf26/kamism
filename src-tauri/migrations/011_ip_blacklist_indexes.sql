-- ip_blacklist 补两个索引。
--
-- 背景：这张表上原本只有一个索引，是**表达式唯一索引**
--   uq_ip_blacklist (COALESCE(merchant_id::text,'global'), ip)
--
-- ⚠️ 曾经把「黑名单 ip 查询是全表扫描」记成待办（理由是「唯一索引前置列是表达式，
--    所以 `WHERE ip = $1` 走不了索引」）。**这个推断是错的，实测推翻了**：
--    Postgres 可以用非前置列做 index cond，`ip` 作为第二列照样走索引。
--    2 万行量级下实测（.workbuddy/batch18-index-bench.sql）：
--      Index Scan using uq_ip_blacklist
--        Index Cond: ((ip)::text = '10.0.0.7'::text)     ← 已经在用索引了
--    教训：**「走不了索引」是推断，不是观测**；不 EXPLAIN 不要写进待办。
--
-- 真正需要索引的是下面第 2 条（商户后台的列表查询，见注释）。

-- ── 1. 热路径：每次激活/校验都要查一次 ──────────────────────────────────────
--   SELECT 1 FROM ip_blacklist WHERE ip = $1 AND (merchant_id IS NULL OR merchant_id = $2) LIMIT 1
--
-- 现状（用表达式索引、ip 作非前置列）不是全表扫描，但代价是**扫描整段索引再按 ip 过滤**
-- —— 20k 行实测 7 buffers / 0.086ms。加了 (ip, merchant_id) 后变成直接定位
-- —— 3 buffers / 0.074ms（Index Only Scan）。
-- 绝对差值很小，但它是 O(索引大小) → O(log n) 的改变：表越大差距越明显，
-- 而这条路是**风控热路径**（public_api.rs 里 fail-closed 的那条），值得先钉住。
CREATE INDEX IF NOT EXISTS idx_ip_blacklist_ip
    ON ip_blacklist (ip, merchant_id);

-- ── 2. 商户后台列表页：这条**确实是全表扫描**，且加 1 号索引救不了 ────────────
--   SELECT id, ip, reason, created_at FROM ip_blacklist
--    WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3
--
-- 为什么 1 号索引帮不上：它的前置列是 ip；而表达式索引里的 merchant_id 被包在
-- COALESCE(...) 里，等值条件 `merchant_id = $1` 匹配不上那个表达式。
-- 20k 行实测，加 1 号索引后这条**仍然是**：
--   Seq Scan on ip_blacklist   Filter: (merchant_id = ...)   Rows Removed by Filter: 20000
--   Buffers: shared hit=186    Execution Time: 3.823 ms      + 一次 quicksort
-- 加上 (merchant_id, created_at DESC) 后：
--   Index Scan using idx_ip_blacklist_merchant   Buffers: shared read=2   Execution Time: 0.077 ms
--   —— 免掉了顺序扫描**和**那次排序（LIMIT/OFFSET 的分页顺序直接由索引给出）。
--
-- 把 created_at DESC 放进索引是为了让 ORDER BY ... DESC LIMIT 不再排序，
-- 分页越深收益越大（否则每页都要把该商户的全部行排一遍）。
CREATE INDEX IF NOT EXISTS idx_ip_blacklist_merchant
    ON ip_blacklist (merchant_id, created_at DESC);
