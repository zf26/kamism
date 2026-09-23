//! Redis 写失败的**处置口径**。
//!
//! ── 为什么要统一 ──────────────────────────────────────────────────────────
//!
//! 第八批治过「吞掉 PG 写错误」，第十批治过「静默失败」。Redis 侧有一个**同构**
//! 但一直没被覆盖的写法 —— 把写命令的结果直接丢掉：
//!
//! ```ignore
//! let _: () = redis.set_ex(&code_key, &code, 600).await.unwrap_or(());
//! ```
//!
//! 全仓库盘出 **21 处**（`grep` 只抓得到其中 4 处；完整扫描器见
//! `.workbuddy/e2e-batch13-redis-write.sh` 的 S4 段）。为什么既不能一刀切换成
//! fail-closed、也不能一刀切「只加一行日志」—— **取决于这个 key 里装的是什么**：
//!
//! | 装的东西 | 写不进之后的后果 | 处置 |
//! |---|---|---|
//! | 业务凭据（验证码、OAuth state） | 用户拿到永远无效的凭据，且被接口告知「已发送」 | [`required`] → **拒绝本次请求** |
//! | 防护措施（防枚举负缓存、防刷冷却、一次性标记） | 防护在该窗口内失效，功能不受影响 | [`protection_lost`] → 放行 + **明确报警** |
//! | 派生态（正缓存、版本号回写、清理、锁释放） | 下次重算 / 自然过期 | [`best_effort`] → 放行 + 留痕 |
//!
//! 判据是一句话：**「写不进去之后，我还能不能对用户说『刚才那件事办成了』？」**
//! 不能 → `required`；能、但某个保护被悄悄撤掉了 → `protection_lost`；能 → `best_effort`。
//!
//! ── 具体案例（本批修的那处） ──────────────────────────────────────────────
//!
//! `auth.rs` 的 `/auth/send-code` 与 `/auth/send-reset-code` 原先都是
//! `set_ex(code_key, code, 600).unwrap_or(())`。后果不是「少了一个缓存」，而是
//! **接口撒谎**：Redis 写不进 → 验证码没存 → 代码继续往下 → 发信（未配 SMTP 时
//! 开发模式直接返回 `Ok`）→ 接口回「验证码已发送」。用户拿到一个**永远验证不通过
//! 的码**，提交时看到「验证码无效或已过期」。
//!
//! 最讽刺的是：**读路径**（`register` / `reset_password` 取码）是一段**手写三分支**，
//! 特意区分「Redis 错误」和「验证码不存在」，还写了注释解释为什么要区分 ——
//! 而写路径连失败都不看。**读路径那套精心处理被写路径架空。**
//!
//! ── 代码库里已有的两个范本 ────────────────────────────────────────────────
//!
//! 说明问题不是「不会做」，而是**没有统一口径**：
//! - `oauth.rs` 写 state：`.map_err(|_| "Redis 错误")?` ← fail-closed（CSRF 凭据）
//! - `jwt.rs::publish_token_version`：`if let Err(e) = r { warn! }` ← fail-open + 留痕
//!
//! ⚠️ **唯一的例外是限流中间件**（`middleware/rate_limit.rs`）：它已经有一套
//! 自己处理过的取舍（fail-open + 每个请求续期补 TTL），而且是热路径。
//! 本批**刻意不动它** —— 见到「吞掉 Redis 失败」不要顺手把它也改了。
//!
//! ⚠️ 为什么用**自由函数**而不是 `AsyncCommands` 的扩展 trait：调用点已经
//! `.await` 出了 `RedisResult`，这里只需要一个「翻译结果 + 记日志」的收口点，
//! 再包一层 async 只会让调用点更长。返回 `Option<T>` / `Result<T, ()>` 让
//! **调用方必须显式决定**要不要据此拒绝请求，而不是默默 `let _ =`。

use redis::RedisResult;

/// 承载**业务凭据**的 Redis 写。失败时记 `error` 并返回 `Err(())`。
///
/// 调用方**必须**据此拒绝当前请求 —— 不要 `let _ =`，那正是本模块要消灭的写法。
///
/// 返回 `Result<T, ()>` 而不是 `bool`：拿到 `Ok` 时还能用里面的值，不必二次查询。
pub fn required<T>(r: RedisResult<T>, what: &str) -> Result<T, ()> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => {
            // 文案刻意点出「绝不能宣称成功」—— 让人一眼看出这是**业务凭据**路径，
            // 而不是顺手加的日志。
            tracing::error!("{}失败（Redis 错误，绝不能对用户宣称成功）: {}", what, e);
            Err(())
        }
    }
}

/// 承载**防护措施**的 Redis 写。失败时放行，但用 `error` 级日志把「防护失效」说出来。
///
/// 为什么用 `error` 而不是 `warn`：限流/防枚举/一次性标记这类东西失效时，
/// **功能完全正常** —— 接口照常返回、测试照常通过、日志里只有一行 warn
/// 很容易被淹没。这一行的唯一价值就是「有人在事后问『为什么防护没生效』时能查到」，
/// 所以抬到 error 级。
pub fn protection_lost<T>(r: RedisResult<T>, what: &str) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::error!(
                "{}失败 —— 该防护在本次窗口内已失效（功能不受影响，方向是安全的）: {}",
                what,
                e
            );
            None
        }
    }
}

/// 承载**派生态**的 Redis 写（正缓存、版本号回写、清理、锁释放）。失败只留痕。
///
/// 「失败即无操作」是**对的**：数据库才是事实来源（见 `jwt.rs` 的读路径），
/// 缓存写不进最多是下次回源重算。但不能静默 —— 否则将来排查
/// 「为什么缓存一直不命中 / 为什么吊销晚了几十秒」会一无所获。
pub fn best_effort<T>(r: RedisResult<T>, what: &str) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("{}失败（仅影响性能或下次重算，不影响本次结论）: {}", what, e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个真实的 `RedisError`，而不是 mock —— 这三个函数不关心错误内容，
    /// 只关心「Err 会不会被吞掉 / 会不会 panic」。用真类型能连「Error 的 Display
    /// 里有换行、会不会把日志格式搞坏」都顺带覆盖到。
    fn boom() -> redis::RedisError {
        redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::Other,
            "boom: connection reset by peer",
        ))
    }

    // ── 契约：Ok 原样透传 ─────────────────────────────────────────────────
    // 这三条钉住「helper 不会把成功结果弄丢」—— 调用方依赖 Ok 里的值。

    #[test]
    fn ok_value_passes_through_all_three() {
        assert_eq!(required(Ok(7u32), "t"), Ok(7));
        assert_eq!(protection_lost(Ok(7u32), "t"), Some(7));
        assert_eq!(best_effort(Ok(7u32), "t"), Some(7));
    }

    // ── 契约：Err 的映射关系 ──────────────────────────────────────────────
    // 这是重点：三档对 Err 的处理**必须不同**，否则调用方没法据此分流。

    #[test]
    fn required_turns_err_into_unit_err() {
        // 调用方靠这个 `Err(())` 决定「拒绝本次请求」。
        assert_eq!(required::<u32>(Err(boom()), "写入注册验证码"), Err(()));
    }

    #[test]
    fn fail_open_helpers_turn_err_into_none() {
        // 这两档放行（不拒绝请求），用 None 表示「没拿到值」。
        assert_eq!(protection_lost::<u32>(Err(boom()), "写入负缓存"), None);
        assert_eq!(best_effort::<u32>(Err(boom()), "续期正缓存"), None);
    }

    // ── 三条都要能处理「错误文本里带换行」而不 panic ──────────────────────
    // Redis 的错误消息可能带换行/控制字符（服务端原文），日志宏必须扛得住。
    #[test]
    fn multiline_error_text_does_not_panic() {
        let e = redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::Other,
            "line1\nline2\r\nline3",
        ));
        assert_eq!(required::<u32>(Err(e), "带换行的错误"), Err(()));
    }
}
