use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PaymentConfig {
    pub id: uuid::Uuid,
    pub channel: String,
    pub name: String,
    pub enabled: bool,
    pub xorpay_aid: Option<String>,
    pub xorpay_app_key: Option<String>,
    pub xorpay_notify_url: Option<String>,
    pub mbdpay_app_id: Option<String>,
    pub mbdpay_app_key: Option<String>,
    pub mbdpay_notify_url: Option<String>,
    pub alipay_app_id: Option<String>,
    pub alipay_private_key: Option<String>,
    pub alipay_public_key: Option<String>,
    pub alipay_notify_url: Option<String>,
    pub alipay_gateway: Option<String>,
    pub alipay_return_url: Option<String>,
    pub extra_config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentConfigPublic {
    pub id: uuid::Uuid,
    pub channel: String,
    pub name: String,
    pub enabled: bool,
    pub alipay_app_id_set: bool,
    pub xorpay_aid_set: bool,
    pub mbdpay_app_id_set: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePaymentConfig {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub xorpay_aid: Option<String>,
    pub xorpay_app_key: Option<String>,
    pub xorpay_notify_url: Option<String>,
    pub mbdpay_app_id: Option<String>,
    pub mbdpay_app_key: Option<String>,
    pub mbdpay_notify_url: Option<String>,
    pub alipay_app_id: Option<String>,
    pub alipay_private_key: Option<String>,
    pub alipay_public_key: Option<String>,
    pub alipay_notify_url: Option<String>,
    pub alipay_gateway: Option<String>,
    pub alipay_return_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TogglePaymentConfig {
    pub enabled: bool,
}
