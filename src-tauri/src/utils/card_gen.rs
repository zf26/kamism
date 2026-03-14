use rand::Rng;

/// 生成卡密，格式：KAMI-XXXX-XXXX-XXXX-XXXX
/// 使用大写字母和数字，去掉易混淆字符 O/0/I/1
pub fn generate_card_code() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let mut gen_segment = |n: usize| -> String {
        (0..n)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect()
    };
    format!(
        "KAMI-{}-{}-{}-{}",
        gen_segment(4),
        gen_segment(4),
        gen_segment(4),
        gen_segment(4)
    )
}

/// 生成 API Key，格式：km_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
pub fn generate_api_key() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    let key: String = (0..32)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();
    format!("km_{}", key)
}

