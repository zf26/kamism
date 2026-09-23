//! 展示用的字符串掩码工具。
//!
//! ── 为什么单独抽出来 ────────────────────────────────────────────────────
//!
//! 全仓库有三处「取前 4 个字符 + `****`」的展示掩码，原先都写成 `&s[..4]` ——
//! 那是**字节**索引。Rust 的字符串索引按字节走，落在一个字符的中间会直接 panic：
//!
//! ```text
//! GET /webhooks（secret 含中文）
//! → [PANIC] end byte index 4 is not a char boundary; it is inside '中'
//!          (bytes 3..6 of string) at src-tauri\src\routes\webhooks.rs:240:27
//!
//! POST /blacklist/devices  {"device_id":"中文"}
//! → [PANIC] end byte index 4 is not a char boundary; it is inside '文'
//!          (bytes 3..6 of string) at src-tauri\src\routes\blacklist.rs:284:37
//! ```
//!
//! 输入是用户可控的（设备号、Webhook secret 都由调用方提供），所以这是**可达的
//! panic**：客户端拿到的是**被直接掐断的连接**（连 500 都没有），服务端只留一行
//! `[PANIC]`。而掩码本来是给人看的 —— 按字符切才是它想表达的意思，
//! 按字节切既会 panic，切出来的也不是「前 4 个字符」。
//!
//! ⚠️ 这些函数只修 panic，**不顺手改既有展示效果**：纯 ASCII 输入下的输出与旧写法
//! 逐字符相同（下面每条单测都钉住了这一点）。

/// 取前 `n` 个**字符**（不是字节）。不足 `n` 个则返回全部。
pub fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 取后 `n` 个**字符**（不是字节）。不足 `n` 个则返回全部。
pub fn suffix_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    s.chars().skip(total.saturating_sub(n)).collect()
}

/// 展示提示：能取满前 `n` 个字符时返回「前 n 字符 + `****`」，否则返回 `****`。
///
/// 与旧写法 `if s.len() >= 4 { format!("{}****", &s[..4]) } else { "****" }`
/// 在纯 ASCII 输入下**逐字符相同** —— 阈值判断也从字节数改成字符数，
/// 否则「中」这种 1 字符 3 字节的输入会走错分支。
pub fn hint(s: &str, n: usize) -> String {
    if s.chars().count() >= n {
        format!("{}****", prefix_chars(s, n))
    } else {
        "****".to_string()
    }
}

/// 保留前 4 后 4 个字符、中间以 `****` 代替；总字符数 ≤ 8 时全部遮成 `*`。
///
/// 旧写法是 `&s[..4]` / `&s[s.len() - 4..]`，两处都是字节索引，都会 panic。
pub fn mask_middle(s: &str) -> String {
    let total = s.chars().count();
    if total <= 8 {
        return "*".repeat(total);
    }
    format!("{}****{}", prefix_chars(s, 4), suffix_chars(s, 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 与旧写法等价（纯 ASCII）────────────────────────────────────────────
    // 这几条是「只修 panic、不改展示」的证据。

    #[test]
    fn hint_matches_old_behavior_on_ascii() {
        assert_eq!(hint("abcd", 4), "abcd****");
        assert_eq!(hint("abcdefgh", 4), "abcd****");
        assert_eq!(hint("abc", 4), "****"); // 旧写法：len() < 4 → "****"
        assert_eq!(hint("", 4), "****");
    }

    #[test]
    fn mask_middle_matches_old_behavior_on_ascii() {
        assert_eq!(mask_middle("abcdefghij"), "abcd****ghij");
        assert_eq!(mask_middle("abcdefghi"), "abcd****fghi");
        assert_eq!(mask_middle("abcdefgh"), "********"); // 旧写法：len() <= 8 → 全遮
        assert_eq!(mask_middle("short"), "*****");
        assert_eq!(mask_middle(""), "");
    }

    // ── 真正要修的东西：多字节输入不得 panic ──────────────────────────────
    // 旧实现（`&s[..4]`）在下面前三行**每一条都会 panic**。

    #[test]
    fn hint_never_panics_on_multibyte() {
        // 「中文」= 2 个字符 / 6 字节。旧写法 len() == 6 >= 4 成立
        // → `&s[..4]` 落在「文」中间 → **panic**（这一行是修复前必崩的输入）。
        // 新写法按字符数判断：2 < 4 → 走「不足」分支。
        assert_eq!(hint("中文", 4), "****");
        // 「中」= 1 个字符 / 3 字节：旧写法 len() == 3 < 4，恰好躲过 panic，
        // 但阈值拿字节数算本身就是错的。
        assert_eq!(hint("中", 4), "****");
        // 字符数够的情况才走前缀分支，且必须按字符切
        assert_eq!(hint("中文abcd", 4), "中文ab****");
        assert_eq!(hint("중국어abcde", 4), "중국어a****");
    }

    /// 这里有一条**刻意**的行为差异，单独钉住，免得日后被当成 bug 改回去。
    ///
    /// 「😀」= 1 个字符 / **恰好 4 个字节**。旧写法 `if s.len() >= 4 { &s[..4] }`
    /// 拿字节数判断，于是 4 >= 4 成立、而 `&s[..4]` 又恰好落在合法边界上 ——
    /// 结果把整个设备号原样显示了出来。新写法按**字符**数判断（1 < 4）→ 输出 `****`。
    ///
    /// 新行为才是对的：不足 4 个字符时就不该显示，否则这个「掩码」把短 id
    /// 全泄漏了。也就是说旧写法除了会 panic，在边界上还会**漏出完整值**。
    #[test]
    fn hint_on_four_byte_single_char_differs_on_purpose() {
        assert_eq!(hint("😀", 4), "****");
        // 同理：3 个字符 9 字节的输入
        assert_eq!(hint("中中中", 4), "****");
    }

    #[test]
    fn mask_middle_never_panics_on_multibyte() {
        // 「中中中」= 9 字节 > 8 → 旧写法 &s[..4] 必然 panic
        assert_eq!(mask_middle("中中中"), "***");
        // 9 个字符 → 前 4 后 4，中间丢 1 个
        assert_eq!(mask_middle("中中中中中中中中中"), "中中中中****中中中中");
        assert_eq!(mask_middle("中文abcdefgh"), "中文ab****efgh");
    }

    #[test]
    fn suffix_chars_handles_short_and_multibyte() {
        assert_eq!(suffix_chars("abcdefghij", 4), "ghij");
        assert_eq!(suffix_chars("ab", 4), "ab");
        assert_eq!(suffix_chars("", 4), "");
        assert_eq!(suffix_chars("中文abc", 4), "文abc");
    }
}
