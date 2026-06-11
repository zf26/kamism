use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OAuthConfig {
    pub id: Uuid,
    pub provider: String,
    pub name: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scopes: String,
    pub enabled: bool,
    pub extra_config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct OAuthConfigPublic {
    pub id: Uuid,
    pub provider: String,
    pub name: String,
    pub enabled: bool,
    pub scopes: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOAuthConfig {
    pub name: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub redirect_uri: Option<String>,
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub scopes: Option<String>,
    pub enabled: Option<bool>,
    pub extra_config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateOAuthProvider {
    pub provider: String,
    pub name: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scopes: Option<String>,
}
