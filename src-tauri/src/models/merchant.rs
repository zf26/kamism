use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub api_key: String,
    pub status: String,
    pub email_verified: bool,
    pub verify_token: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MerchantPublic {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub api_key: String,
    pub status: String,
    pub email_verified: bool,
    pub created_at: DateTime<Utc>,
}

impl From<Merchant> for MerchantPublic {
    fn from(m: Merchant) -> Self {
        MerchantPublic {
            id: m.id,
            username: m.username,
            email: m.email,
            api_key: m.api_key,
            status: m.status,
            email_verified: m.email_verified,
            created_at: m.created_at,
        }
    }
}

