use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Activation {
    pub id: Uuid,
    pub card_id: Uuid,
    pub app_id: Uuid,
    #[sqlx(rename = "device_id_encrypted")]
    pub device_id: String,
    pub device_name: Option<String>,
    pub ip_address: Option<String>,
    pub activated_at: DateTime<Utc>,
    pub last_verified_at: DateTime<Utc>,
}

