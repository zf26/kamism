use crate::middleware::auth::{auth_middleware, AppState};
use crate::utils::db_guard;
use crate::utils::jwt::Claims;
use crate::models::payment_config::PaymentConfig;
use axum::{
    extract::{Query, State},
    middleware,
    routing::{get, post},
    Extension, Json, Router,
};
use base64::Engine;
use md5;
use rsa::{
    pkcs1::DecodeRsaPrivateKey,
    pkcs1v15::{SigningKey, VerifyingKey},
    pkcs8::{DecodePrivateKey, DecodePublicKey},
    signature::{RandomizedSigner, SignatureEncoding, Verifier},
    RsaPrivateKey, RsaPublicKey,
};
use sha2::Sha256;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// XorPay 配置
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct XorPayConfig {
    pub aid: String,
    pub app_key: String,
    pub notify_url: String,
}

impl XorPayConfig {
    pub fn from_db(cfg: &PaymentConfig) -> Self {
        Self {
            aid: cfg.xorpay_aid.clone().unwrap_or_default(),
            app_key: cfg.xorpay_app_key.clone().unwrap_or_default(),
            notify_url: cfg.xorpay_notify_url.clone()
                .unwrap_or_else(|| "http://localhost:9527/pay/notify".to_string()),
        }
    }

    fn sign(&self, name: &str, pay_type: &str, price: &str, order_id: &str) -> String {
        let data = format!(
            "{}{}{}{}{}{}",
            name, pay_type, price, order_id, self.notify_url, self.app_key
        );
        format!("{:x}", md5::compute(data.as_bytes()))
    }

    pub fn verify_sign(
        &self,
        aoid: &str,
        order_id: &str,
        pay_price: &str,
        pay_time: &str,
    ) -> String {
        let data = format!(
            "{}{}{}{}{}",
            aoid, order_id, pay_price, pay_time, self.app_key
        );
        format!("{:x}", md5::compute(data.as_bytes()))
    }

    pub fn is_configured(&self) -> bool {
        !self.aid.is_empty() && !self.app_key.is_empty()
    }

    pub async fn create_order(
        &self,
        client: &reqwest::Client,
        order_id: &str,
        name: &str,
        price: &str,
        pay_type: &str,
    ) -> Result<XorPayCreateResult, String> {
        let sign = self.sign(name, pay_type, price, order_id);

        let params = [
            ("name", name),
            ("pay_type", pay_type),
            ("price", price),
            ("order_id", order_id),
            ("notify_url", &self.notify_url),
            ("sign", &sign),
        ];

        let resp = client
            .post(format!("https://xorpay.com/api/pay/{}", self.aid))
            .form(&params)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("XorPay 请求失败: {}", e))?;

        #[derive(serde::Deserialize)]
        struct XorPayResp {
            status: String,
            info: Option<XorPayQr>,
            expires_in: Option<i32>,
            aoid: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct XorPayQr {
            qr: String,
        }

        let xor_resp: XorPayResp = resp
            .json()
            .await
            .map_err(|e| format!("XorPay 响应解析失败: {}", e))?;

        if xor_resp.status != "ok" {
            return Err(format!("XorPay 错误: {}", xor_resp.status));
        }

        Ok(XorPayCreateResult {
            pay_url: xor_resp.info.as_ref().map(|i| i.qr.clone()),
            expires_in: xor_resp.expires_in.unwrap_or(7200),
            charge_id: xor_resp.aoid,
        })
    }
}

#[derive(Clone)]
pub struct XorPayCreateResult {
    pub pay_url: Option<String>,
    pub expires_in: i32,
    pub charge_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 支付宝电脑网站支付配置
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AlipayConfig {
    pub app_id: String,
    pub app_private_key: String,
    pub alipay_public_key: String,
    pub notify_url: String,
    pub gateway: String,
    pub return_url: String,
}

impl AlipayConfig {
    pub fn from_db(cfg: &PaymentConfig) -> Self {
        Self {
            app_id: cfg.alipay_app_id.clone().unwrap_or_default(),
            app_private_key: cfg.alipay_private_key.clone().unwrap_or_default(),
            alipay_public_key: cfg.alipay_public_key.clone().unwrap_or_default(),
            notify_url: cfg.alipay_notify_url.clone()
                .unwrap_or_else(|| "http://localhost:9527/pay/notify".to_string()),
            gateway: cfg.alipay_gateway.clone()
                .unwrap_or_else(|| "https://openapi.alipay.com/gateway.do".to_string()),
            return_url: cfg.alipay_return_url.clone()
                .unwrap_or_else(|| "http://localhost:1420/dashboard".to_string()),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.app_id.is_empty()
            && !self.app_private_key.is_empty()
            && !self.alipay_public_key.is_empty()
    }

    /// 自动拼接 PEM 格式，解析支付宝私钥
    fn sign(&self, content: &str) -> Result<String, String> {
        tracing::info!("[sign] 被调用，content 长度 {} 字节", content.len());
        let mut raw = self.app_private_key.trim().to_string();

        // 去掉用户复制时可能带上的引号
        raw = raw.trim_matches(|c| c == '"' || c == '\'').to_string();

        // 直接用 base64 解码为 DER，再尝试 PKCS#8 / PKCS#1 DER 解析。
        // 这样完全绕过 pem-rfc7468 的行宽限制（它要求每行 ≤ 64 字符，
        // 但支付宝工具导出的私钥常为整行 base64，交给 PEM parser 会报
        // "invalid Base64 encoding"）。
        let der_bytes = if raw.starts_with("-----BEGIN") {
            // 有 PEM 头尾：从 PEM 中提取 base64 并解码
            Self::extract_b64_from_pem(&raw)
                .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64)
                    .map_err(|e| format!("私钥 base64 解码失败: {}", e)))
        } else {
            // 无 PEM 头尾：去掉所有空白后直接解码
            let oneline: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(&oneline)
                .map_err(|e| format!("私钥 base64 解码失败: {}", e))
        }.map_err(|e| format!("解析支付宝私钥失败: {}", e))?;

        tracing::info!("[sign] DER 字节长度: {}", der_bytes.len());
        tracing::info!("[sign] DER 首字节: 0x{:02x} (0x30=PKCS8/PKCS1, 0x2x=裸DSA签名等)", der_bytes.first().copied().unwrap_or(0));
        // 打印前 16 字节的十六进制，方便对照 DER 结构
        tracing::info!("[sign] DER hex (前16字节): {}", der_bytes.iter().take(16).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" "));

        tracing::info!("[sign] 正在解析私钥 DER...");
        let private_key = RsaPrivateKey::from_pkcs8_der(&der_bytes)
            .or_else(|_| RsaPrivateKey::from_pkcs1_der(&der_bytes))
            .map_err(|e| {
                tracing::error!("[sign] 私钥 DER 解析失败，首字节: 0x{:02x}", der_bytes.first().copied().unwrap_or(0));
                format!("解析支付宝私钥失败: {}（DER 首字节 0x{:02x}，若为 0x30 则应为 PKCS#8/PKCS#1 格式）",
                    e, der_bytes.first().copied().unwrap_or(0))
            })?;
        tracing::info!("[sign] 私钥解析成功，开始签名...");

        tracing::info!("[sign] 正在签名 content 长度 {} 字节...", content.len());
        tracing::info!("[sign] content hex: {}", content.bytes().map(|b| format!("{:02x}", b)).collect::<String>());
        let signing_key = SigningKey::<Sha256>::new(private_key);
        let signature = signing_key.sign_with_rng(&mut rand::thread_rng(), content.as_bytes());
        tracing::info!("[sign] 签名完成");
        let sig_bytes = signature.to_bytes();
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(&sig_bytes);
        tracing::info!("[sign] 签名 base64: {}", sig_b64);
        tracing::info!("[sign] 签名 hex: {}", sig_bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>());

        Ok(base64::engine::general_purpose::STANDARD.encode(sig_bytes))
    }

    /// 从已有 PEM 中提取 base64 内容（去掉头尾行和所有换行）
    fn extract_b64_from_pem(pem: &str) -> Result<String, String> {
        Ok(pem.lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect())
    }

    /// 验证签名（RSA-PSS + SHA256，支付宝 RSA2 标准）
    pub fn verify_sign(&self, sign: &str, content: &str) -> bool {
        let trimmed = self.alipay_public_key.trim();

        // 提取干净 base64 字符串
        let b64 = if trimmed.starts_with("-----BEGIN") {
            Self::extract_b64_from_pem(trimmed).unwrap_or_default()
        } else {
            trimmed.chars().filter(|c| !c.is_whitespace()).collect()
        };

        // base64 解码为 DER
        let der_bytes = match base64::engine::general_purpose::STANDARD.decode(&b64) {
            Ok(b) => b,
            Err(_) => return false,
        };

        // 公钥 SPKI/DER 解析
        let public_key: RsaPublicKey = match RsaPublicKey::from_public_key_der(&der_bytes) {
            Ok(k) => k,
            Err(_) => return false,
        };

        let decoded_sign = match base64::engine::general_purpose::STANDARD.decode(sign) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let signature = match rsa::pkcs1v15::Signature::try_from(decoded_sign.as_slice()) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let verifying_key = VerifyingKey::<Sha256>::new(public_key);
        verifying_key.verify(content.as_bytes(), &signature).is_ok()
    }

    /// 创建支付宝电脑网站支付（返回支付页面 URL，前端打开后展示二维码）
    pub async fn create_order(
        &self,
        _client: &reqwest::Client,
        order_id: &str,
        subject: &str,
        total_amount: &str,
    ) -> Result<AlipayCreateResult, String> {
        tracing::info!("[alipay create_order] order_id={}", order_id);
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let biz_content = json!({
            "out_trade_no": order_id,
            "product_code": "FAST_INSTANT_TRADE_PAY",
            "total_amount": total_amount,
            "subject": subject,
            "qr_pay_mode": "1",
        })
        .to_string();

        let params = vec![
            ("app_id", self.app_id.clone()),
            ("method", "alipay.trade.page.pay".to_string()),
            ("charset", "utf-8".to_string()),
            ("sign_type", "RSA2".to_string()),
            ("timestamp", timestamp),
            ("version", "1.0".to_string()),
            ("notify_url", self.notify_url.clone()),
            ("return_url", self.return_url.clone()),
            ("biz_content", biz_content),
        ];

        // ── 按字典序排列，生成验签内容 ──
        let mut sorted = params.clone();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let content: String = sorted
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        tracing::info!("[alipay] 排序后验签内容: {}", content);

        let sign = self.sign(&content)?;

        // ── 构建签名的 URL（参数值 URL 编码）──
        let query: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let pay_url = format!("{}?{}&sign={}", self.gateway, query, urlencoding::encode(&sign));

        tracing::info!("[alipay create_order] pay_url 生成完成");

        Ok(AlipayCreateResult {
            pay_url: Some(pay_url),
            pay_html: None,
            expires_in: 7200,
            charge_id: Some(order_id.to_string()),
        })
    }
}

#[derive(Clone)]
pub struct AlipayCreateResult {
    pub pay_url: Option<String>,
    pub pay_html: Option<String>,
    pub expires_in: i32,
    pub charge_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// MbdPay 配置
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct MbdPayConfig {
    pub app_id: String,
    pub app_key: String,
    pub notify_url: String,
}

impl MbdPayConfig {
    pub fn from_db(cfg: &PaymentConfig) -> Self {
        Self {
            app_id: cfg.mbdpay_app_id.clone().unwrap_or_default(),
            app_key: cfg.mbdpay_app_key.clone().unwrap_or_default(),
            notify_url: cfg.mbdpay_notify_url.clone()
                .unwrap_or_else(|| "http://localhost:9527/pay/notify".to_string()),
        }
    }

    /// 面包多签名：key1=value1&key2=value2&...&key={app_key}，然后 MD5
    fn sign(&self, params: &[(String, String)]) -> String {
        let mut sorted: Vec<_> = params
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .cloned()
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let query = sorted
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        let data = format!("{}&key={}", query, self.app_key);
        format!("{:x}", md5::compute(data.as_bytes()))
    }

    pub fn is_configured(&self) -> bool {
        !self.app_id.is_empty() && !self.app_key.is_empty()
    }

    /// 微信 H5 支付
    async fn wx_h5(
        &self,
        client: &reqwest::Client,
        order_id: &str,
        description: &str,
        amount_cents: i32,
    ) -> Result<MbdPayCreateResult, String> {
        let params = vec![
            ("app_id".to_string(), self.app_id.clone()),
            ("channel".to_string(), "h5".to_string()),
            ("description".to_string(), description.to_string()),
            ("out_trade_no".to_string(), order_id.to_string()),
            ("amount_total".to_string(), amount_cents.to_string()),
        ];
        let sign = self.sign(&params);

        #[derive(serde::Deserialize)]
        struct MbdWxResp {
            h5_url: Option<String>,
            error: Option<String>,
        }

        let resp = client
            .post("https://newapi.mbd.pub/release/wx/prepay")
            .json(&serde_json::json!({
                "channel": "h5",
                "app_id": self.app_id,
                "description": description,
                "out_trade_no": order_id,
                "amount_total": amount_cents,
                "sign": sign,
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("MbdPay 微信H5请求失败: {}", e))?;

        let body: MbdWxResp = resp
            .json()
            .await
            .map_err(|e| format!("MbdPay 响应解析失败: {}", e))?;

        if let Some(err) = body.error {
            return Err(format!("MbdPay 微信H5错误: {}", err));
        }

        Ok(MbdPayCreateResult {
            pay_url: body.h5_url,
            pay_html: None,
            expires_in: 7200,
            charge_id: None,
        })
    }

    /// 支付宝扫码
    async fn alipay_qr(
        &self,
        client: &reqwest::Client,
        order_id: &str,
        description: &str,
        amount_cents: i32,
    ) -> Result<MbdPayCreateResult, String> {
        let params = vec![
            ("app_id".to_string(), self.app_id.clone()),
            ("description".to_string(), description.to_string()),
            ("out_trade_no".to_string(), order_id.to_string()),
            ("amount_total".to_string(), amount_cents.to_string()),
        ];
        let sign = self.sign(&params);

        #[derive(serde::Deserialize)]
        struct MbdAliResp {
            #[serde(rename = "qr_code")]
            qr_code: Option<String>,
            html: Option<String>,
            error: Option<String>,
        }

        let resp = client
            .post("https://newapi.mbd.pub/release/ali/precreate")
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "description": description,
                "out_trade_no": order_id,
                "amount_total": amount_cents,
                "sign": sign,
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("MbdPay 支付宝请求失败: {}", e))?;

        let body: MbdAliResp = resp
            .json()
            .await
            .map_err(|e| format!("MbdPay 响应解析失败: {}", e))?;

        if let Some(err) = body.error {
            return Err(format!("MbdPay 支付宝错误: {}", err));
        }

        Ok(MbdPayCreateResult {
            pay_url: body.qr_code,
            pay_html: body.html,
            expires_in: 7200,
            charge_id: None,
        })
    }

    pub async fn create_order(
        &self,
        client: &reqwest::Client,
        order_id: &str,
        name: &str,
        price: &str,
        pay_type: &str,
    ) -> Result<MbdPayCreateResult, String> {
        let amount_cents = (price.parse::<f64>().unwrap_or(0.0) * 100.0) as i32;
        match pay_type {
            "wechat" => self.wx_h5(client, order_id, name, amount_cents).await,
            "alipay" => self.alipay_qr(client, order_id, name, amount_cents).await,
            _ => Err(format!("pay_type 不支持: {}", pay_type)),
        }
    }

    /// 验签 webhook，返回 (是否有效, order_id, 实付金额字符串)
    pub fn verify_notify(
        &self,
        body: &serde_json::Value,
    ) -> Result<(bool, String, String), String> {
        let typ = body.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let data = body
            .get("data")
            .and_then(|v| v.as_object())
            .ok_or("MbdPay webhook 缺少 data 字段")?;

        let out_trade_no = data
            .get("out_trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let amount = data.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
        let amount_str = format!("{:.2}", amount as f64 / 100.0);
        let sign = body.get("sign").and_then(|v| v.as_str()).unwrap_or("");

        // type=charge_succeeded&data[amount]=...&data[out_trade_no]=...（字典序）
        let params = vec![
            ("type".to_string(), typ.to_string()),
            ("data[amount]".to_string(), amount.to_string()),
            ("data[out_trade_no]".to_string(), out_trade_no.to_string()),
        ];
        let mut sorted = params.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let query = sorted
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        let expected = format!(
            "{:x}",
            md5::compute(format!("{}&key={}", query, self.app_key).as_bytes())
        );

        if !expected.eq_ignore_ascii_case(sign) {
            return Err("MbdPay 签名验证失败".to_string());
        }

        Ok((true, out_trade_no.to_string(), amount_str))
    }
}

#[derive(Clone)]
pub struct MbdPayCreateResult {
    pub pay_url: Option<String>,
    pub pay_html: Option<String>,
    pub expires_in: i32,
    pub charge_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 双通道统一状态
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PayConfig {
    pub enabled_channel: String,
}

impl PayConfig {
    /// 从数据库读取支付配置
    pub async fn get_channel_config(
        app_state: &AppState,
        channel: &str,
    ) -> Option<PaymentConfig> {
        app_state.get_payment_config(channel).await
    }
}

#[derive(Clone)]
pub struct PayState {
    pub app_state: AppState,
}

// ─────────────────────────────────────────────────────────────────────────────
// 路由注册
// ─────────────────────────────────────────────────────────────────────────────

pub fn payments_router(state: AppState) -> Router<AppState> {
    let pay_state = PayState {
        app_state: state.clone(),
    };

    Router::new()
        .nest("/pay/auth", authed_payment_router(pay_state.clone()))
        .route("/pay/notify", post(pay_notify))
        // ── 已删除：`GET /pay/query/:order_id` ──────────────────────────────
        //
        // 这里原本注册的是一条**永远无法工作**的路由，两个独立的缺陷叠在一起：
        //
        // 1. 路径参数写成了 `{order_id}`（axum 0.8 语法）。本项目用 axum 0.7，
        //    它只认 `:order_id`，花括号会被当成**字面量** → 注册出的是
        //    `/pay/query/%7Border_id%7D`，任何真实路径都匹配不上。
        //    实测：`GET /pay/query/X` → 404 且无 `allow` 头（= 路由不存在），
        //    而 `/pay/notify` → 405 带 `allow: POST`（= 路由存在、方法不对）。
        //    两种状态码的区别就是判别「路由到底注册上没有」的依据。
        //
        // 2. 它挂在 `/pay` 根上，而 `auth_middleware` 只作用于 `nest("/pay/auth", ...)`
        //    内部。所以 handler 的 `Extension<Claims>` 永远没有来源。
        //    把路径语法改对之后立刻实测到 500，响应体是：
        //      `Missing request extension: Extension of type Claims was not found.
        //       Perhaps you forgot to add it?`
        //    —— 带合法 token 也一样 500，因为中间件根本没机会跑。
        //
        // 为什么删而不是修：功能上与 `GET /pay/auth/status?order_id=` 完全重复，
        // 且后者是它的**严格超集**（8 个字段 vs 3 个字段）并且有正确的 auth。
        // 全仓库（前端 / 文档 / 测试）**零调用**。
        // 一个没人用、功能重复、且自身坏掉的接口，删掉比修好更有价值 ——
        // 修好它等于凭空多维护一个与 authed 版语义重叠的入口。
        //
        // ⚠️ 若将来要恢复无前缀的查询口，必须同时解决「谁往请求里塞 Claims」。
        // 直接加 `.route_layer(auth_middleware)` 是最小改法，但要先想清楚
        // 为什么需要两个查询口。
        .with_state(pay_state)
}

fn authed_payment_router(state: PayState) -> Router<PayState> {
    Router::new()
        .route("/create", post(create_order))
        .route("/orders", get(list_orders))
        .route("/status", get(get_order_status))
        .route("/cancel", post(cancel_order))
        .route_layer(middleware::from_fn_with_state(
            state.app_state.clone(),
            auth_middleware,
        ))
}

// ─────────────────────────────────────────────────────────────────────────────
// 辅助函数
// ─────────────────────────────────────────────────────────────────────────────

fn get_merchant_id(claims: &Claims) -> Result<Uuid, Json<Value>> {
    match Uuid::parse_str(&claims.sub) {
        Ok(id) => Ok(id),
        Err(_) => Err(Json(json!({"success": false, "message": "无效用户ID"}))),
    }
}

fn get_plan_price(expires_days: Option<i32>) -> (String, String) {
    match expires_days {
        Some(days) if days > 0 => (
            format!("{:.2}", days as f64),
            format!("KamiSM 专业版 {} 天续费", days),
        ),
        _ => ("365.00".to_string(), "KamiSM 专业版（永久）".to_string()),
    }
}

/// 核对通道回调的实付金额与订单金额是否一致。
///
/// 三个通道给的金额**都是「元」**（MbdPay 的 `verify_notify` 已把分除以 100 并格式化成
/// 两位小数；XorPay 的 `pay_price`、支付宝的 `total_amount` 本身就是元），
/// 但字符串形态不统一（`"365"` / `"365.0"` / `"365.00"`），所以按**数值**比较。
///
/// 解析失败（含金额字段缺失被上游兜底成 `"0.00"` 的情况）一律判为不匹配：
/// 在无法核对金额时宁可拒绝入账，也不要把订单标成已支付。
fn amounts_match(order_amount: &str, notified: &str) -> bool {
    match (
        order_amount.trim().parse::<f64>(),
        notified.trim().parse::<f64>(),
    ) {
        (Ok(a), Ok(b)) => (a - b).abs() < 0.005,
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求类型
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateOrderRequest {
    pub pay_type: String, // 'wechat' | 'alipay'
    /// 套餐 ID（UUID），优先使用；从 subscription_plans 表查询
    pub plan_id: Option<Uuid>,
    /// 后备：按 expires_days 计算价格（plan_id 不存在时使用）
    pub expires_days: Option<i32>,
    /// 可选：强制指定支付渠道（mbdpay | xorpay | alipay）
    pub channel: Option<String>,
}

#[derive(Deserialize)]
pub struct ListOrdersQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub channel: Option<String>,
}

#[derive(Deserialize)]
pub struct OrderStatusQuery {
    pub order_id: String,
}

#[derive(Deserialize)]
pub struct CancelOrderRequest {
    pub order_id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// 创建订单
// ─────────────────────────────────────────────────────────────────────────────

async fn create_order(
    State(state): State<PayState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateOrderRequest>,
) -> Json<Value> {
    let merchant_id = match get_merchant_id(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    let pay_type = match body.pay_type.as_str() {
        "wechat" | "alipay" => body.pay_type.clone(),
        _ => return Json(json!({"success": false, "message": "pay_type 仅支持 wechat / alipay"})),
    };

    // 优先从 subscription_plans 表查套餐，否则按 expires_days 兜底
    //
    // ⚠️⚠️ 这里曾经用 `.unwrap_or(None)`，是**整个仓库里方向最危险的一处**，
    // 因为它把「基础设施故障」直接变成了「按客户端说的价格算」：
    //
    //   1. 正常路径：`plan_id` → 查 `subscription_plans` → 价格由**服务端**决定
    //   2. 出错路径（旧代码）：查询失败 → `None` → 走 `get_plan_price(body.expires_days)`
    //      —— 而 `body.expires_days` 是**请求体里客户端自己传的**
    //
    // 也就是说：数据库抖一下，下单价格就从「套餐价 30 天」变成「客户端说多少天就多少天」。
    // 这不是理论风险 —— 攻击者只要挑服务抖动的窗口下单（或者更直接：想办法让这条
    // 查询超时），就能用任意价格买到套餐。而且**日志上什么都没有**。
    //
    // 所以这里不能 fallback：查不出来就拒绝下单。宁可用户点不了「立即支付」，
    // 也不能按不可信的价格生成订单。
    //
    // 注意 `None => { get_plan_price(...) }` 那个分支是**另一种情况**：
    // 请求里根本没传 `plan_id`（老客户端 / /pay 的简单模式），
    // 那条路径的价格本来就不是从表里查的，属于正常的业务分支，保持不变。
    let (price, name, plan_days) = match body.plan_id {
        Some(plan_id) => {
            let plan = match db_guard::optional(
                sqlx::query_as::<_, crate::models::subscription_plan::SubscriptionPlan>(
                    "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, \
                     badge, highlight, sort_order, enabled, created_at, updated_at \
                     FROM subscription_plans WHERE id = $1 AND enabled = TRUE"
                )
                    .bind(plan_id)
                    .fetch_optional(&state.app_state.pool),
                "查询订阅套餐（下单定价）",
            )
            .await
            {
                // 查到套餐 —— 价格由服务端决定
                db_guard::QueryOutcome::Found(p) => Some(p),
                // 套餐不存在或已下架 —— 这是正常的业务结论，可以走 expires_days 兜底
                db_guard::QueryOutcome::NotFound => None,
                // 查询失败 —— **绝不能**当成「套餐不存在」去走客户端传的 expires_days
                db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
            };
            match plan {
                Some(p) => {
                    let (price, name) = (
                        format!("{:.2}", p.price),
                        match p.days {
                            Some(d) => format!("KamiSM 专业版 {} 天续费", d),
                            None => "KamiSM 专业版（永久）".to_string(),
                        },
                    );
                    (price, name, p.days)
                }
                None => {
                    let (price, name) = get_plan_price(body.expires_days);
                    (price, name, body.expires_days)
                }
            }
        }
        None => {
            let (price, name) = get_plan_price(body.expires_days);
            (price, name, body.expires_days)
        }
    };
    // 从套餐查到的天数优先于请求传入的 days
    let final_expires_days = plan_days.or(body.expires_days);
    let price_f64: f64 = price.parse().unwrap_or(0.0);
    let order_id = format!("KAMI{}", chrono::Utc::now().timestamp_millis());
    let now = chrono::Utc::now();

    // 渠道选择：优先用请求指定的，其次查 DB 中已启用的，再 fallback 到 env
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：查询失败 → 当成「没有任何启用的渠道」→
    // fallback 到硬编码的 `"alipay"`。后果是**创建出渠道错误的订单**：
    // 用户在界面上点了微信支付，生成的却是支付宝订单，付款时会一头雾水。
    // 而且这条失败路径没有日志，运维只会看到「有用户说支付渠道不对」。
    //
    // 只有当请求**明确指定了渠道**时才允许继续（那是客户端的明确意图，
    // 不需要查库）；否则查不出来就拒绝。
    let channel_str = match body.channel.clone() {
        Some(c) => c,
        None => {
            let enabled = match db_guard::optional(
                sqlx::query_as::<_, (String,)>("SELECT channel FROM payment_configs WHERE enabled = TRUE LIMIT 1")
                    .fetch_optional(&state.app_state.pool),
                "查询已启用的支付渠道",
            )
            .await
            {
                db_guard::QueryOutcome::Found(c) => Some(c),
                // 确实一个都没启用 —— 这是配置问题，走下面的 alipay 兜底保持原有行为
                db_guard::QueryOutcome::NotFound => None,
                // 查询失败 —— 不能拿一个"猜"的渠道去下单
                db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
            };
            enabled.map(|r| r.0).unwrap_or_else(|| "alipay".to_string())
        }
    };
    let channel: &str = &channel_str;

    // 保存订单记录
    let res = sqlx::query(
        r#"INSERT INTO payments
           (merchant_id, order_id, pay_channel, pay_type, amount, plan, expires_days, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           RETURNING id"#,
    )
    .bind(merchant_id)
    .bind(&order_id)
    .bind(channel)
    .bind(&pay_type)
    .bind(price_f64)
    .bind("pro")
    .bind(final_expires_days)
    .bind(now)
    .bind(now)
    .fetch_one(&state.app_state.pool)
    .await;

    if res.is_err() {
        tracing::error!("创建订单失败: {:?}", res.err());
        return Json(json!({"success": false, "message": "创建订单失败"}));
    }

    let client = reqwest::Client::new();
    let channel_name: &str;
    let (pay_url, pay_html, charge_id, expires_in) = match channel {
        "mbdpay" => {
            let db_cfg = PayConfig::get_channel_config(&state.app_state, channel).await
                .map(|c| MbdPayConfig::from_db(&c));
            let cfg = match db_cfg {
                Some(c) => c,
                None => return Json(json!({"success": false, "message": "MbdPay 未配置"})),
            };
            match cfg.create_order(&client, &order_id, &name, &price, &pay_type).await {
                Ok(r) => {
                    channel_name = "MbdPay";
                    (r.pay_url, r.pay_html, r.charge_id, r.expires_in)
                }
                Err(e) => return Json(json!({"success": false, "message": e})),
            }
        }
        "alipay" => {
            let db_cfg = PayConfig::get_channel_config(&state.app_state, channel).await
                .map(|c| AlipayConfig::from_db(&c));
            let cfg = match db_cfg {
                Some(c) => c,
                None => return Json(json!({"success": false, "message": "支付宝电脑网站支付未配置"})),
            };
            tracing::info!("[handler] 准备调用 AlipayConfig::create_order, order_id={}", order_id);
            match cfg.create_order(&client, &order_id, &name, &price).await {
                Ok(r) => {
                    channel_name = "Alipay";
                    (r.pay_url, r.pay_html, r.charge_id, r.expires_in)
                }
                Err(e) => return Json(json!({"success": false, "message": e})),
            }
        }
        _ => {
            let db_cfg = PayConfig::get_channel_config(&state.app_state, channel).await
                .map(|c| XorPayConfig::from_db(&c));
            let cfg = match db_cfg {
                Some(c) => c,
                None => return Json(json!({"success": false, "message": "XorPay 未配置"})),
            };
            match cfg.create_order(&client, &order_id, &name, &price, &pay_type).await {
                Ok(r) => {
                    channel_name = "XorPay";
                    (r.pay_url, None, r.charge_id, r.expires_in)
                }
                Err(e) => return Json(json!({"success": false, "message": e})),
            }
        }
    };

    // 记录 charge_id
    if let Some(cid) = charge_id {
        let col = match channel {
            "mbdpay" => "mbdpay_charge_id",
            "alipay" => "alipay_trade_no",
            _ => "xorpay_aoid",
        };
        // ⚠️ 这里曾经是 `let _ =`，写失败没有任何痕迹。
        // 这一行是**唯一**记录支付渠道流水号的地方（三列目前全仓库只写不读，
        // 是人工对账/查单用的）—— 写失败就只剩 order_id 能对，等于对账能力降级。
        //
        // 不能因为这里失败就返回失败：订单在渠道侧**已经创建**，pay_url 必须交给用户，
        // 否则用户付了钱却拿不到跳转地址。所以取舍是「照常返回 + error 留痕」。
        let recorded = sqlx::query(&format!(
            "UPDATE payments SET {} = $1 WHERE order_id = $2",
            col
        ))
        .bind(&cid)
        .bind(&order_id)
        .execute(&state.app_state.pool)
        .await;

        match recorded {
            Ok(r) if r.rows_affected() > 0 => {}
            // 订单行是本函数上面刚 INSERT 成功的，为 0 说明它被并发删掉了 —— 罕见但要留痕
            Ok(_) => tracing::error!(
                "记录支付渠道流水号失败：没有匹配的订单行（rows_affected=0），该订单无法用流水号人工对账: order_id={} col={}",
                order_id,
                col
            ),
            Err(e) => tracing::error!(
                "记录支付渠道流水号失败，该订单无法用流水号人工对账: order_id={} col={} err={}",
                order_id,
                col,
                e
            ),
        }
    }

    tracing::info!(
        "创建{}订单: order_id={}, price={}",
        channel_name,
        order_id,
        price
    );

    Json(json!({
        "success": true,
        "data": {
            "order_id": order_id,
            "pay_url": pay_url,
            "pay_html": pay_html,
            "expires_in": expires_in,
            "price": price,
            "pay_type": pay_type,
            "plan": "pro",
            "expires_days": body.expires_days,
            "channel": channel,
        }
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// 订单列表
// ─────────────────────────────────────────────────────────────────────────────

type OrderRow = (
    String,
    String,
    String,
    String,
    String,
    Option<i32>,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
);

async fn list_orders(
    State(state): State<PayState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<ListOrdersQuery>,
) -> Json<Value> {
    let merchant_id = match get_merchant_id(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);
    let offset = (page - 1) * page_size;

    // ⚠️ 曾用 `.unwrap_or_default()` + `.unwrap_or((0,))`。
    // 订单历史是最不该"静默变空"的一类页面：用户打开「我的订单」看到一片空白，
    // 第一反应是**「我的付款记录丢了？」**——这比一个明确的错误提示糟糕得多。
    let orders: Vec<OrderRow> = {
        match sqlx::query_as::<_, OrderRow>(
            "SELECT order_id, pay_channel, pay_type, amount::text, status, expires_days, created_at, pay_time \
             FROM payments WHERE merchant_id = $1 \
             AND ($2::text IS NULL OR pay_channel = $2) \
             ORDER BY created_at DESC LIMIT $3 OFFSET $4"
        )
            .bind(merchant_id)
            .bind(&q.channel)
            .bind(page_size)
            .bind(offset)
            .fetch_all(&state.app_state.pool)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("查询订单列表失败: merchant_id={} err={}", merchant_id, e);
                return db_guard::server_busy();
            }
        }
    };

    let total = match db_guard::scalar(
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM payments WHERE merchant_id = $1")
            .bind(merchant_id)
            .fetch_one(&state.app_state.pool),
        "统计订单总数",
    )
    .await
    {
        db_guard::ScalarOutcome::Found(t) => t,
        db_guard::ScalarOutcome::Failed => return db_guard::server_busy(),
    };

    let data: Vec<Value> = orders
        .into_iter()
        .map(
            |(
                order_id,
                pay_channel,
                pay_type,
                amount,
                status,
                expires_days,
                created_at,
                pay_time,
            )| {
                json!({
                    "order_id": order_id,
                    "pay_channel": pay_channel,
                    "pay_type": pay_type,
                    "amount": amount,
                    "status": status,
                    "expires_days": expires_days,
                    "created_at": created_at.to_rfc3339(),
                    "pay_time": pay_time.map(|t| t.to_rfc3339()),
                })
            },
        )
        .collect();

    Json(json!({
        "success": true,
        "data": data,
        "total": total.0,
        "page": page,
        "page_size": page_size,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// 查询订单状态
// ─────────────────────────────────────────────────────────────────────────────

async fn get_order_status(
    State(state): State<PayState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<OrderStatusQuery>,
) -> Json<Value> {
    let merchant_id = match get_merchant_id(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    // ⚠️ 曾用 `.unwrap_or(None)`。这个接口是**前端支付后的轮询口**（通常是每秒一次），
    // 静默失败的后果特别糟：
    //   用户刚在支付宝里付完钱 → 前端开始轮询 → 恰好这几次查询失败 →
    //   前端收到「订单不存在」→ 界面显示**支付失败**。
    //   而钱其实已经扣了，订单也会被回调改成已支付 ——
    //   用户看到的是"钱没了、订单也没了"，会立刻来投诉。
    //
    // 把 `NotFound`（订单号真的不对）和 `Failed`（查不了）分开，
    // 前端就至少能区分「这个订单号不存在」和「状态暂时查不到，再等等」。
    let row = match db_guard::optional(
        sqlx::query_as::<_, OrderRow>(
            "SELECT order_id, pay_channel, pay_type, amount::text, status, expires_days, created_at, pay_time
             FROM payments WHERE order_id = $1 AND merchant_id = $2",
        )
        .bind(&q.order_id)
        .bind(merchant_id)
        .fetch_optional(&state.app_state.pool),
        "查询订单状态",
    )
    .await
    {
        db_guard::QueryOutcome::Found(r) => Some(r),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    match row {
        Some((
            order_id,
            pay_channel,
            pay_type,
            amount,
            status,
            expires_days,
            created_at,
            pay_time,
        )) => Json(json!({
            "success": true,
            "data": {
                "order_id": order_id,
                "pay_channel": pay_channel,
                "pay_type": pay_type,
                "amount": amount,
                "status": status,
                "expires_days": expires_days,
                "created_at": created_at.to_rfc3339(),
                "pay_time": pay_time.map(|t| t.to_rfc3339()),
            }
        })),
        None => Json(json!({"success": false, "message": "订单不存在"})),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 支付回调（多通道共用）
// ─────────────────────────────────────────────────────────────────────────────

async fn pay_notify(
    State(state): State<PayState>,
    body: String,
) -> &'static str {
    // 兼容 JSON 和 form-urlencoded 两种格式（MbdPay 发 JSON，支付宝/XorPay 发 form）
    let body_value: serde_json::Value = if body.trim().starts_with('{') {
        serde_json::from_str(&body).unwrap_or_default()
    } else {
        let mut map = std::collections::BTreeMap::new();
        for pair in body.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                let key = urlencoding::decode(k).unwrap_or_else(|_| std::borrow::Cow::Borrowed(k)).to_string();
                let value = urlencoding::decode(v).unwrap_or_else(|_| std::borrow::Cow::Borrowed(v)).to_string();
                map.insert(key, value);
            }
        }
        serde_json::to_value(&map).unwrap_or_default()
    };

    let body = body_value;
    // 判定是哪个通道的回调
    let is_mbdpay = body.get("type").is_some() && body.get("data").is_some();
    let is_alipay = body.get("sign_type").is_some() && body.get("sign").is_some();

    let (channel_name, order_id, pay_price) = if is_mbdpay {
        let cfg = PayConfig::get_channel_config(&state.app_state, "mbdpay")
            .await
            .map(|c| MbdPayConfig::from_db(&c));
        let cfg = match cfg {
            Some(c) => c,
            None => {
                tracing::warn!("MbdPay 回调但未配置");
                return "error";
            }
        };
        match cfg.verify_notify(&body) {
            Ok((_, oid, price)) => ("MbdPay", oid, price),
            Err(e) => {
                tracing::warn!("MbdPay 签名验证失败: {}", e);
                return "sign_error";
            }
        }
    } else if is_alipay {
        let cfg = PayConfig::get_channel_config(&state.app_state, "alipay")
            .await
            .map(|c| AlipayConfig::from_db(&c));
        let cfg = match cfg {
            Some(c) => c,
            None => {
                tracing::warn!("支付宝回调但未配置");
                return "error";
            }
        };

        // 提取回调参数
        let _sign_type = body.get("sign_type").and_then(|v| v.as_str()).unwrap_or("");
        let sign = body.get("sign").and_then(|v| v.as_str()).unwrap_or("");
        let out_trade_no = body
            .get("out_trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let trade_status = body
            .get("trade_status")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 只处理交易成功的回调
        if trade_status != "TRADE_SUCCESS" && trade_status != "TRADE_FINISHED" {
            tracing::warn!("支付宝回调交易状态非成功: {}", trade_status);
            return "success";
        }

        // 验证签名：排除 sign 和 sign_type 字段后按字典序拼接再验证
        let mut params: Vec<(String, String)> = body
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(k, _)| *k != "sign" && *k != "sign_type")
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                    .collect()
            })
            .unwrap_or_default();
        params.sort_by(|a, b| a.0.cmp(&b.0));
        let content: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");

        tracing::debug!("[alipay callback] 验签 content: {}", content);

        // 支付宝回调签名验证
        if !cfg.verify_sign(sign, &content) {
            tracing::warn!("支付宝签名验证失败: out_trade_no={}", out_trade_no);
            return "sign_error";
        }

        let total_amount = body
            .get("total_amount")
            .and_then(|v| v.as_str())
            .unwrap_or("0.00");
        ("Alipay", out_trade_no.to_string(), total_amount.to_string())
    } else {
        let cfg = PayConfig::get_channel_config(&state.app_state, "xorpay")
            .await
            .map(|c| XorPayConfig::from_db(&c));
        let cfg = match cfg {
            Some(c) => c,
            None => {
                tracing::warn!("XorPay 回调但未配置");
                return "error";
            }
        };
        let aoid = body.get("aoid").and_then(|v| v.as_str()).unwrap_or("");
        let order_id = body.get("order_id").and_then(|v| v.as_str()).unwrap_or("");
        let pay_price = body.get("pay_price").and_then(|v| v.as_str()).unwrap_or("");
        let pay_time = body.get("pay_time").and_then(|v| v.as_str()).unwrap_or("");
        let sign = body.get("sign").and_then(|v| v.as_str()).unwrap_or("");

        let expected = cfg.verify_sign(aoid, order_id, pay_price, pay_time);
        if expected != sign {
            tracing::warn!("XorPay 签名验证失败: order_id={}", order_id);
            return "sign_error";
        }
        ("XorPay", order_id.to_string(), pay_price.to_string())
    };

    // ── 事务内完成「锁行 → 幂等判定 → 金额核对 → 更新 → 升级」全流程 ──
    //
    // 为什么必须在事务内、且要 FOR UPDATE：
    // 支付通道会重复投递回调（网络重试、通道自身的重推策略），同一笔订单可能有两个
    // 请求几乎同时到达。旧写法是「事务外查一次 status，事务里不带条件地 UPDATE」——
    // 两个并发请求都能通过事务外那次检查，然后各自给商户加一遍套餐时长。
    // FOR UPDATE 把同一订单的并发回调串行化：后到的会阻塞到前者提交，
    // 再读到的就是 status = 'paid'，于是走幂等分支直接确认返回。
    let mut tx = match state.app_state.pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[{}] 开启事务失败: {}", channel_name, e);
            return "error";
        }
    };

    // 一次查询同时充当：行锁 + 幂等判定 + 金额核对 + 升级所需字段。
    // amount 用 ::text 取回，避免为了一个字段给 sqlx 打开 bigdecimal feature。
    let order: Result<Option<(Uuid, String, String, Option<i32>, Uuid, String)>, sqlx::Error> =
        sqlx::query_as(
            "SELECT id, status, amount::text, expires_days, merchant_id, plan
             FROM payments WHERE order_id = $1 FOR UPDATE",
        )
        .bind(&order_id)
        .fetch_optional(&mut *tx)
        .await;

    let (payment_id, order_amount, expires_days, merchant_id, plan) = match order {
        Err(e) => {
            tracing::error!("[{}] 查询订单失败: {}", channel_name, e);
            let _ = tx.rollback().await;
            return "error";
        }
        Ok(None) => {
            tracing::warn!("[{}] 回调订单不存在: {}", channel_name, order_id);
            let _ = tx.rollback().await;
            return "order_not_found";
        }
        Ok(Some((_, ref status, _, _, _, _))) if status == "paid" => {
            // 幂等：这笔订单已经处理过（通常是通道重复投递）—— 确认收到，但不重复加时长
            tracing::info!(
                "[{}] 重复回调，订单已支付，幂等跳过: order_id={}",
                channel_name,
                order_id
            );
            let _ = tx.rollback().await;
            return "ok";
        }
        Ok(Some((id, _, amount, days, mch, p))) => (id, amount, days, mch, p),
    };

    // 金额核对：实付金额必须与下单时的订单金额一致
    if !amounts_match(&order_amount, &pay_price) {
        tracing::error!(
            "[{}] 回调金额与订单金额不符，拒绝入账: order_id={}, 订单金额={}, 回调金额={}",
            channel_name,
            order_id,
            order_amount,
            pay_price
        );
        let _ = tx.rollback().await;
        return "amount_mismatch";
    }

    let notify_json = serde_json::to_string(&body).unwrap_or_default();
    // WHERE 再带一次 status 条件作双保险：即使 FOR UPDATE 因故没锁住，
    // 也不会把已支付的订单再写一遍（此时 rows_affected 为 0）。
    let updated = sqlx::query(
        "UPDATE payments SET status = 'paid', pay_price = $1, pay_time = $2, notify_data = $3, updated_at = NOW()
         WHERE id = $4 AND status <> 'paid'",
    )
    .bind(&pay_price)
    .bind(chrono::Utc::now())
    .bind(&notify_json)
    .bind(payment_id)
    .execute(&mut *tx)
    .await;

    match updated {
        Err(e) => {
            tracing::error!("[{}] 更新支付状态失败: {}", channel_name, e);
            let _ = tx.rollback().await;
            return "error";
        }
        Ok(ref r) if r.rows_affected() == 0 => {
            // 并发回调抢先完成，本次视为重复投递
            tracing::info!(
                "[{}] 订单状态已被并发回调置为已支付，幂等跳过: order_id={}",
                channel_name,
                order_id
            );
            let _ = tx.rollback().await;
            return "ok";
        }
        Ok(_) => {}
    }

    // 套餐升级：只有 pro 套餐才需要动商户
    let mut need_upgrade_msg = false;
    if plan == "pro" {
        let update_result = if let Some(days) = expires_days {
            sqlx::query(
                "UPDATE merchants SET plan = 'pro', plan_expires_at = COALESCE(plan_expires_at, NOW()) + ($1 || ' days')::INTERVAL, updated_at = NOW() WHERE id = $2",
            )
            .bind(days.to_string())
            .bind(merchant_id)
            .execute(&mut *tx)
            .await
        } else {
            sqlx::query(
                "UPDATE merchants SET plan = 'pro', plan_expires_at = NULL, updated_at = NOW() WHERE id = $1",
            )
            .bind(merchant_id)
            .execute(&mut *tx)
            .await
        };

        if let Err(e) = update_result {
            tracing::error!("[{}] 更新商户套餐失败: {}", channel_name, e);
            let _ = tx.rollback().await;
            return "error";
        }

        need_upgrade_msg = true;
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("[{}] 提交事务失败: {}", channel_name, e);
        return "error";
    }

    // ── 事务成功后发布升级消息（避免事务回滚但消息已发出）──
    if need_upgrade_msg {
        if let Err(e) = crate::utils::mq::publish_upgrade(
            &state.app_state.mq_channel,
            &merchant_id.to_string(),
        )
        .await
        {
            tracing::error!("[{}] 发布升级恢复消息失败: {}", channel_name, e);
        }
    }

    tracing::info!(
        "[{}] 支付成功: order_id={}, price={}",
        channel_name,
        order_id,
        pay_price
    );
    "ok"
}

// ─────────────────────────────────────────────────────────────────────────────
// 主动查询（商户前端轮询兜底）
//
// `pay_query` 已删除 —— 理由见 `payments_router` 里的注释。
// 这个功能由 `GET /pay/auth/status?order_id=` 承担（`authed_payment_router` 内，
// 字段是这里的超集，且 auth 正确）。
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// 取消订单
// ─────────────────────────────────────────────────────────────────────────────

async fn cancel_order(
    State(state): State<PayState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CancelOrderRequest>,
) -> Json<Value> {
    let merchant_id = match get_merchant_id(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    // 只允许取消 pending 状态的订单，且必须属于当前用户
    let result = sqlx::query(
        "UPDATE payments SET status = 'cancelled', updated_at = NOW()
         WHERE order_id = $1 AND merchant_id = $2 AND status = 'pending'",
    )
    .bind(&body.order_id)
    .bind(merchant_id)
    .execute(&state.app_state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!("订单已取消: order_id={}", body.order_id);
            Json(json!({"success": true, "message": "订单已取消"}))
        }
        Ok(_) => Json(json!({"success": false, "message": "订单不存在或无法取消"})),
        Err(e) => {
            tracing::error!("取消订单失败: {}", e);
            Json(json!({"success": false, "message": "取消失败"}))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::amounts_match;

    #[test]
    fn amounts_match_accepts_equivalent_formats() {
        // 订单侧是 DECIMAL(10,2) 取出的 "365.00"，回调侧小数位数可能不同
        assert!(amounts_match("365.00", "365"));
        assert!(amounts_match("365.00", "365.0"));
        assert!(amounts_match("365.00", "365.00"));
        assert!(amounts_match("0.01", "0.01"));
        // form-urlencoded 解析后可能残留空白
        assert!(amounts_match("365.00", " 365.00 "));
    }

    #[test]
    fn amounts_match_rejects_mismatch_and_garbage() {
        // 少付不该入账
        assert!(!amounts_match("365.00", "0.01"));
        // 多付同样不该 —— 金额对不上说明订单与回调不是同一笔
        assert!(!amounts_match("365.00", "3650.00"));
        // MbdPay 回调缺 data[amount] 时，上游兜底成 0 分 → "0.00"
        assert!(!amounts_match("365.00", "0.00"));
        // 通道完全没给金额字段
        assert!(!amounts_match("365.00", ""));
        assert!(!amounts_match("365.00", "abc"));
    }

    #[test]
    fn amounts_match_tolerates_sub_cent_rounding() {
        // 半分钱以内视为相等，避免浮点表示差异造成误拒
        assert!(amounts_match("365.00", "365.004"));
        // 但差一分钱就是不一致
        assert!(!amounts_match("365.00", "365.01"));
    }
}
