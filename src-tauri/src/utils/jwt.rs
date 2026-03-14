use anyhow::Result;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub email: String,
    pub exp: i64,
    pub iat: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RefreshClaims {
    pub sub: String,       // user id
    pub role: String,
    pub email: String,
    pub token_type: String, // 固定为 "refresh"
    pub exp: i64,
    pub iat: i64,
}

/// 生成 Access Token（2小时有效期）
pub fn generate_token(user_id: &Uuid, role: &str, email: &str, secret: &str) -> Result<String> {
    let now = Utc::now();
    let exp = now + Duration::hours(2);
    let claims = Claims {
        sub: user_id.to_string(),
        role: role.to_string(),
        email: email.to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

/// 生成 Refresh Token（7天有效期，使用独立密钥前缀区分）
pub fn generate_refresh_token(user_id: &Uuid, role: &str, email: &str, secret: &str) -> Result<String> {
    let now = Utc::now();
    let exp = now + Duration::days(7);
    let refresh_secret = format!("{}:refresh", secret);
    let claims = RefreshClaims {
        sub: user_id.to_string(),
        role: role.to_string(),
        email: email.to_string(),
        token_type: "refresh".to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(refresh_secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn verify_token(token: &str, secret: &str) -> Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(token_data.claims)
}

pub fn verify_refresh_token(token: &str, secret: &str) -> Result<RefreshClaims> {
    let refresh_secret = format!("{}:refresh", secret);
    let token_data = decode::<RefreshClaims>(
        token,
        &DecodingKey::from_secret(refresh_secret.as_bytes()),
        &Validation::default(),
    )?;
    // 确保是 refresh token 而非 access token
    if token_data.claims.token_type != "refresh" {
        return Err(anyhow::anyhow!("不是有效的 Refresh Token"));
    }
    Ok(token_data.claims)
}
