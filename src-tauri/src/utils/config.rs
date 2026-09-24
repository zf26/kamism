//! 启动期的配置校验。
//!
//! 拦的是同一类错误：**把 `env.example` 里的占位符当成配置用**。
//!
//! 这类问题在运行时是**完全静默**的 —— `JWT_SECRET=change_me_xxx` 照样能签发和
//! 校验 token（只是任何人都能伪造出来），`ADMIN_PASSWORD=Admin@123456` 照样能登录
//! （只是这个口令公开在仓库里）。**唯一的拦截点就是启动**。
//!
//! 所以这里刻意选「宁可启动失败」：带一把可预测的签名密钥上生产，
//! 比启动失败严重得多。（对齐 `env.example` 里 `MASTER_KEY` 的既有约定：
//! 格式不对就直接拒绝启动，不带一把新密钥去读旧数据。）
//!
//! 注意：校验的值和实际使用的值是**同一个**（不做 trim）。否则会出现
//! 「校验的是 A、用的是 B」这种两头都对的假象。

use std::collections::HashSet;

/// 一眼能看出「还没填」的占位符。比对时忽略大小写。
///
/// `example` / `todo` 这类词也在里面：真实的随机密钥（hex / base64）里
/// 不可能自然出现它们。
const PLACEHOLDER_MARKERS: &[&str] = &[
    "change_me",
    "changeme",
    "change-me",
    "placeholder",
    "your_secret",
    "your-secret",
    "your_password",
    "your-password",
    "your_jwt",
    "example",
    "todo",
];

/// 公开文档 / 教程里出现过的默认口令。
///
/// 为什么要单独列一张表：`Admin@123456` 的长相**完全合法** —— 12 位、
/// 大小写字母 + 数字 + 符号四类齐全，任何「强度检查」都抓不住它。
/// 它的问题不在形状，而在于**它是公开的**（本项目自己的 `env.example`
/// 和历史版本用的就是这个）。这种事只能靠黑名单认出来。
const WEAK_PASSWORDS: &[&str] = &[
    "admin@123456",
    "admin123456",
    "admin@123",
    "admin123",
    "administrator",
    "password",
    "password123",
    "password@123",
    "123456789012",
    "qwerty123456",
    "changeme",
    "letmein",
    "kamism@123456",
    "test@123456",
    "root@123456",
];

pub const MIN_JWT_SECRET_LEN: usize = 32;
pub const MIN_ADMIN_PASSWORD_LEN: usize = 12;
/// 到这个长度就不再要求字符类别了（`openssl rand -hex 32` 出来的 64 位 hex
/// 只有小写字母和数字两类，但没人会说它弱）。**长度本来就比字符类别重要。**
const LEN_ENOUGH_NO_CLASS_RULE: usize = 24;
/// 至少这么多种不同字符 —— 挡的是「先按 32 次 a」「abababab…」这种凑长度的。
const MIN_DISTINCT_CHARS: usize = 8;

fn find_placeholder(s: &str) -> Option<&'static str> {
    let lower = s.to_lowercase();
    PLACEHOLDER_MARKERS
        .iter()
        .copied()
        .find(|m| lower.contains(m))
}

/// 校验 JWT 签名密钥；`Err` 里是给部署者看的原因。
pub fn validate_jwt_secret(secret: &str) -> Result<(), String> {
    if secret.is_empty() {
        return Err("值为空。生成一个：openssl rand -hex 32".to_string());
    }
    if let Some(marker) = find_placeholder(secret) {
        return Err(format!(
            "看起来还是 env.example 里的占位符（命中 `{}`）。占位符是可预测的，\
             等于把签名密钥公开 —— 任何人都能伪造出任意用户的 token。\
             生成一个真的：openssl rand -hex 32",
            marker
        ));
    }
    let len = secret.chars().count();
    if len < MIN_JWT_SECRET_LEN {
        return Err(format!(
            "长度 {} 少于 {} 位，太短。生成一个：openssl rand -hex 32",
            len, MIN_JWT_SECRET_LEN
        ));
    }
    let distinct = secret.chars().collect::<HashSet<_>>().len();
    if distinct < MIN_DISTINCT_CHARS {
        return Err(format!(
            "只有 {} 种不同字符，像是重复粘贴出来的，不像随机密钥",
            distinct
        ));
    }
    Ok(())
}

/// 校验初始管理员口令。
///
/// `admin_email` 用来挡住「口令就是邮箱前缀」。
pub fn validate_admin_password(password: &str, admin_email: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err(format!(
            "值为空。请设置一个至少 {} 位的强口令",
            MIN_ADMIN_PASSWORD_LEN
        ));
    }
    let lower = password.to_lowercase();

    // ⚠️ 错误信息里不要带上口令本身：它会被打进启动日志。
    if WEAK_PASSWORDS.contains(&lower.as_str()) {
        return Err(
            "这是公开的默认 / 教程口令（env.example 和历史版本里用的就是这个），\
             必须换掉 —— 它不是「强度不够」，而是**任何人都知道**"
                .to_string(),
        );
    }
    if let Some(marker) = find_placeholder(password) {
        return Err(format!(
            "看起来还是占位符（命中 `{}`），请填一个真的口令",
            marker
        ));
    }
    let len = password.chars().count();
    if len < MIN_ADMIN_PASSWORD_LEN {
        return Err(format!(
            "长度 {} 少于 {} 位",
            len, MIN_ADMIN_PASSWORD_LEN
        ));
    }

    let local = admin_email
        .split('@')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if local.chars().count() >= 4 && lower == local {
        return Err("口令不能就是管理员邮箱的前缀".to_string());
    }

    if len < LEN_ENOUGH_NO_CLASS_RULE {
        let classes = [
            password.chars().any(|c| c.is_ascii_lowercase()),
            password.chars().any(|c| c.is_ascii_uppercase()),
            password.chars().any(|c| c.is_ascii_digit()),
            password.chars().any(|c| !c.is_ascii_alphanumeric()),
        ]
        .iter()
        .filter(|x| **x)
        .count();
        if classes < 3 {
            return Err(
                "至少要有「小写 / 大写 / 数字 / 符号」中的 3 类（或长度达到 24 位以上）"
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一把像样的 64 位 hex（16 种不同字符）
    const GOOD_SECRET: &str =
        "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    // ── JWT_SECRET ────────────────────────────────────────────────────────

    #[test]
    fn jwt_secret_accepts_a_random_looking_value() {
        assert!(validate_jwt_secret(GOOD_SECRET).is_ok());
    }

    #[test]
    fn jwt_secret_rejects_empty() {
        let err = validate_jwt_secret("").unwrap_err();
        assert!(err.contains("为空"), "实际: {err}");
    }

    /// 这条就是本批要防的东西：env.example 里那一行原样照抄。
    #[test]
    fn jwt_secret_rejects_the_env_example_placeholder() {
        let err = validate_jwt_secret("change_me_jwt_secret_at_least_32_chars").unwrap_err();
        assert!(err.contains("占位符"), "实际: {err}");
    }

    #[test]
    fn jwt_secret_rejects_other_placeholders() {
        for v in [
            "CHANGE_ME_but_long_enough_to_pass_length_check",
            "your-secret-here-and-make-it-long-enough",
            "placeholder_value_put_something_real_here",
            "TODO_fill_this_in_before_deploying",
        ] {
            assert!(validate_jwt_secret(v).is_err(), "应拒绝: {v}");
        }
    }

    #[test]
    fn jwt_secret_rejects_too_short() {
        let err = validate_jwt_secret("9f2c1a7be4d6083f").unwrap_err();
        assert!(err.contains("长度"), "实际: {err}");
    }

    /// 长度够了但只有一种字符 —— 凑出来的，不是随机的。
    #[test]
    fn jwt_secret_rejects_low_variety() {
        let err = validate_jwt_secret(&"a".repeat(64)).unwrap_err();
        assert!(err.contains("不同字符"), "实际: {err}");
    }

    /// 32 位是最低线（正好 32 要放行）。
    #[test]
    fn jwt_secret_accepts_exactly_32_chars() {
        assert!(validate_jwt_secret("9f2c1a7be4d6083f5a1c9e2b7d4f8a30").is_ok());
    }

    // ── ADMIN_PASSWORD ────────────────────────────────────────────────────

    #[test]
    fn admin_password_accepts_a_strong_one() {
        assert!(validate_admin_password("Zx7#mQ2w!Lp9", "admin@example.com").is_ok());
    }

    /// 本批的靶心：这个口令**四类字符齐全、长度 12**，任何强度检查都放行，
    /// 只有黑名单认得出来。它必须被拒 —— 否则本批等于没做。
    #[test]
    fn admin_password_rejects_the_published_default() {
        let err = validate_admin_password("Admin@123456", "admin@example.com").unwrap_err();
        assert!(
            err.contains("公开"),
            "Admin@123456 必须因「公开默认口令」被拒，实际: {err}"
        );
    }

    #[test]
    fn admin_password_rejects_other_known_weak_ones() {
        for v in ["password123", "qwerty123456", "letmein", "administrator"] {
            assert!(
                validate_admin_password(v, "admin@example.com").is_err(),
                "应拒绝: {v}"
            );
        }
    }

    #[test]
    fn admin_password_rejects_empty() {
        let err = validate_admin_password("", "admin@example.com").unwrap_err();
        assert!(err.contains("为空"), "实际: {err}");
    }

    #[test]
    fn admin_password_rejects_short() {
        let err = validate_admin_password("Ab1!xyz", "admin@example.com").unwrap_err();
        assert!(err.contains("长度"), "实际: {err}");
    }

    /// 够长但只有一类字符。
    #[test]
    fn admin_password_rejects_single_class() {
        let err = validate_admin_password("abcdefghijklmno", "admin@example.com").unwrap_err();
        assert!(err.contains("3 类"), "实际: {err}");
    }

    /// 24 位以上不再要求字符类别 —— 否则 `openssl rand -hex 32`
    /// （我们自己在错误信息里推荐的那条命令）生成的口令会被拒。
    #[test]
    fn admin_password_long_random_hex_is_ok() {
        assert!(validate_admin_password(GOOD_SECRET, "admin@example.com").is_ok());
    }

    #[test]
    fn admin_password_rejects_email_local_part() {
        let err = validate_admin_password("kamism#2026x", "kamism#2026x@test.local").unwrap_err();
        assert!(err.contains("邮箱"), "实际: {err}");
    }

    /// 错误信息不能把口令本身带出去 —— 它会被打进启动日志。
    ///
    /// 覆盖每一条 Err 分支：黑名单 / 占位符 / 太短 / 字符类别不足。
    #[test]
    fn error_messages_never_echo_the_password() {
        let cases = [
            ("Admin@123456", "admin@example.com"),          // 黑名单
            ("Ab1!xy", "admin@example.com"),                // 太短
            ("abcdefghijklmno", "admin@example.com"),       // 类别不足
            ("change_me_password_please", "admin@example.com"), // 占位符
        ];
        for (pw, email) in cases {
            let err = validate_admin_password(pw, email).unwrap_err();
            assert!(
                !err.contains(pw),
                "错误信息里带出了口令 `{pw}`: {err}"
            );
        }
    }
}
