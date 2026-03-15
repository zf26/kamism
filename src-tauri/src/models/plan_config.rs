use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PlanConfig {
    pub id: Uuid,
    pub plan: String,
    pub label: String,
    pub max_apps: i32,
    pub max_cards: i32,
    pub max_devices: i32,
    pub max_gen_once: i32,
    pub updated_at: DateTime<Utc>,
}

