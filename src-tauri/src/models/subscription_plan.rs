use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SubscriptionPlan {
    pub id: Uuid,
    pub plan: String,
    pub name: String,
    pub days: Option<i32>,
    pub price: f64,
    #[sqlx(default)]
    pub original_price: Option<f64>,
    #[sqlx(default)]
    pub badge: Option<String>,
    pub highlight: bool,
    pub sort_order: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePlanRequest {
    pub plan: String,
    pub name: String,
    pub days: Option<i32>,
    pub price: f64,
    pub original_price: Option<f64>,
    pub badge: Option<String>,
    pub highlight: Option<bool>,
    pub sort_order: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePlanRequest {
    pub name: Option<String>,
    pub days: Option<i32>,
    pub price: Option<f64>,
    pub original_price: Option<f64>,
    pub badge: Option<String>,
    pub highlight: Option<bool>,
    pub sort_order: Option<i32>,
    pub enabled: Option<bool>,
}
