use crate::middleware::auth::{auth_middleware, AppState};
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
        .route("/pay/query/{order_id}", get(pay_query))
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
    let (price, name) = match body.plan_id {
        Some(plan_id) => {
            let plan: Option<crate::models::subscription_plan::SubscriptionPlan> =
                sqlx::query_as(
                    "SELECT id, plan, name, days, price::float8 AS price, original_price::float8 AS original_price, \
                     badge, highlight, sort_order, enabled, created_at, updated_at \
                     FROM subscription_plans WHERE id = $1 AND enabled = TRUE"
                )
                    .bind(plan_id)
                    .fetch_optional(&state.app_state.pool)
                    .await
                    .unwrap_or(None);
            match plan {
                Some(p) => (
                    format!("{:.2}", p.price),
                    match p.days {
                        Some(d) => format!("KamiSM 专业版 {} 天续费", d),
                        None => "KamiSM 专业版（永久）".to_string(),
                    },
                ),
                None => get_plan_price(body.expires_days),
            }
        }
        None => get_plan_price(body.expires_days),
    };
    let price_f64: f64 = price.parse().unwrap_or(0.0);
    let order_id = format!("KAMI{}", chrono::Utc::now().timestamp_millis());
    let now = chrono::Utc::now();

    // 渠道选择：优先用请求指定的，其次查 DB 中已启用的，再 fallback 到 env
    let channel_str = match body.channel.clone() {
        Some(c) => c,
        None => {
            let enabled: Option<(String,)> = sqlx::query_as(
                "SELECT channel FROM payment_configs WHERE enabled = TRUE LIMIT 1",
            )
            .fetch_optional(&state.app_state.pool)
            .await
            .unwrap_or(None);
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
    .bind(body.expires_days)
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
        let _ = sqlx::query(&format!(
            "UPDATE payments SET {} = $1 WHERE order_id = $2",
            col
        ))
        .bind(&cid)
        .bind(&order_id)
        .execute(&state.app_state.pool)
        .await;
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

    let channel_filter = q
        .channel
        .as_ref()
        .map(|c| format!(" AND pay_channel = '{}'", c));

    let orders: Vec<OrderRow> = {
        let query = format!(
            "SELECT order_id, pay_channel, pay_type, amount::text, status, expires_days, created_at, pay_time
             FROM payments WHERE merchant_id = $1{} ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            channel_filter.as_deref().unwrap_or(""),
        );
        sqlx::query_as::<_, OrderRow>(&query)
            .bind(merchant_id)
            .bind(page_size)
            .bind(offset)
            .fetch_all(&state.app_state.pool)
            .await
            .unwrap_or_default()
    };

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM payments WHERE merchant_id = $1")
        .bind(merchant_id)
        .fetch_one(&state.app_state.pool)
        .await
        .unwrap_or((0,));

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

    let row: Option<OrderRow> = sqlx::query_as(
        "SELECT order_id, pay_channel, pay_type, amount::text, status, expires_days, created_at, pay_time
         FROM payments WHERE order_id = $1 AND merchant_id = $2",
    )
    .bind(&q.order_id)
    .bind(merchant_id)
    .fetch_optional(&state.app_state.pool)
    .await
    .unwrap_or(None);

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

    // 幂等检查
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id::text, status FROM payments WHERE order_id = $1")
            .bind(&order_id)
            .fetch_optional(&state.app_state.pool)
            .await
            .unwrap_or(None);

    let payment_id = match row {
        Some((id, s)) if s != "paid" => id,
        Some(_) => return "ok",
        None => {
            tracing::warn!("[{}] 回调订单不存在: {}", channel_name, order_id);
            return "order_not_found";
        }
    };

    // 更新订单
    let notify_json = serde_json::to_string(&body).unwrap_or_default();
    let _ = sqlx::query(
        "UPDATE payments SET status = 'paid', pay_price = $1, pay_time = $2, notify_data = $3, updated_at = NOW() WHERE id = $4",
    )
    .bind(&pay_price)
    .bind(chrono::Utc::now())
    .bind(&notify_json)
    .bind(payment_id)
    .execute(&state.app_state.pool)
    .await;

    // 更新商户套餐
    let order_row: Option<(Uuid, String, Option<i32>)> = sqlx::query_as(
        "SELECT merchant_id::text, plan, expires_days FROM payments WHERE order_id = $1",
    )
    .bind(&order_id)
    .fetch_optional(&state.app_state.pool)
    .await
    .unwrap_or(None);

    if let Some((merchant_id, plan, expires_days)) = order_row {
        if plan == "pro" {
            if expires_days.is_some() {
                sqlx::query(
                    "UPDATE merchants SET plan = 'pro', plan_expires_at = COALESCE(plan_expires_at, NOW()) + ($1 || ' days')::INTERVAL, updated_at = NOW() WHERE id = $2",
                )
                .bind(expires_days.unwrap().to_string())
                .bind(merchant_id)
                .execute(&state.app_state.pool)
                .await
                .ok();
            } else {
                sqlx::query(
                    "UPDATE merchants SET plan = 'pro', plan_expires_at = NULL, updated_at = NOW() WHERE id = $1",
                )
                .bind(merchant_id)
                .execute(&state.app_state.pool)
                .await
                .ok();
            }

            if let Err(e) = crate::utils::mq::publish_upgrade(
                &state.app_state.mq_channel,
                &merchant_id.to_string(),
            )
            .await
            {
                tracing::error!("发布升级恢复消息失败: {}", e);
            }
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
// ─────────────────────────────────────────────────────────────────────────────

async fn pay_query(
    State(state): State<PayState>,
    Extension(claims): Extension<Claims>,
    axum::extract::Path(order_id): axum::extract::Path<String>,
) -> Json<Value> {
    let merchant_id = match get_merchant_id(&claims) {
        Ok(id) => id,
        Err(e) => return e,
    };

    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT order_id, status, pay_channel FROM payments WHERE order_id = $1 AND merchant_id = $2",
    )
    .bind(&order_id)
    .bind(merchant_id)
    .fetch_optional(&state.app_state.pool)
    .await
    .unwrap_or(None);

    match row {
        Some((order_id, status, pay_channel)) => Json(
            json!({"success": true, "data": { "order_id": order_id, "status": status, "pay_channel": pay_channel }}),
        ),
        None => Json(json!({"success": false, "message": "订单不存在"})),
    }
}

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
