use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Card {
    pub id: Uuid,
    pub app_id: Uuid,
    pub merchant_id: Uuid,
    #[sqlx(rename = "code_encrypted")]
    pub code: String,
    pub duration_days: i32,
    pub max_devices: i32,
    pub status: String,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

