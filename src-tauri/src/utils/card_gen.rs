use rand::Rng;

const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 生成卡密。
///
/// - `prefix`：自定义前缀，如 `"VIP"` → `"VIP-XXXX-XXXX-XXXX-XXXX"`；
///   留空或 None 时使用默认前缀 `"KAMI"`。
/// - `segment_count`：段数（不含前缀），范围 1–8，默认 4。
/// - `segment_len`：每段字符数，范围 2–8，默认 4。
///
/// 字符集去掉了易混淆的 O/0/I/1。
pub fn generate_card_code_with_format(
    prefix: Option<&str>,
    segment_count: usize,
    segment_len: usize,
) -> String {
    let prefix = prefix.unwrap_or("KAMI");
    let seg_count = segment_count.clamp(1, 8);
    let seg_len   = segment_len.clamp(2, 8);

    let mut rng = rand::thread_rng();
    let segments: Vec<String> = (0..seg_count)
        .map(|_| {
            (0..seg_len)
                .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
                .collect()
        })
        .collect();

    format!("{}-{}", prefix, segments.join("-"))
}

/// 默认格式快捷函数：`KAMI-XXXX-XXXX-XXXX-XXXX`
pub fn generate_card_code() -> String {
    generate_card_code_with_format(None, 4, 4)
}

/// 生成 API Key，格式：km_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
pub fn generate_api_key() -> String {
    const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    let key: String = (0..32)
        .map(|_| LOWER[rng.gen_range(0..LOWER.len())] as char)
        .collect();
    format!("km_{}", key)
}
