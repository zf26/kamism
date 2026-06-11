use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone)]
pub struct GitHubOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

impl GitHubOAuthConfig {
    pub fn from_env() -> Self {
        Self {
            client_id: env::var("GITHUB_CLIENT_ID").unwrap_or_default(),
            client_secret: env::var("GITHUB_CLIENT_SECRET").unwrap_or_default(),
            redirect_uri: env::var("GITHUB_REDIRECT_URI")
                .unwrap_or_else(|_| "http://localhost:9527/oauth/github/callback".to_string()),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.client_id.is_empty() && !self.client_secret.is_empty()
    }

    /// Generates the GitHub OAuth authorization URL and returns (url, state).
    pub fn get_authorization_url(&self) -> Option<(String, String)> {
        if !self.is_configured() {
            return None;
        }

        let state = generate_random_state();

        let url = format!(
            "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=user:email%20read:user&state={}",
            urlencoding::encode(&self.client_id),
            urlencoding::encode(&self.redirect_uri),
            urlencoding::encode(&state),
        );

        Some((url, state))
    }
}

fn generate_random_state() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Debug, Deserialize)]
pub struct GitHubTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
}

/// Exchange authorization code for access token
pub async fn exchange_code_for_token(
    config: &GitHubOAuthConfig,
    code: &str,
) -> anyhow::Result<String> {
    let params = [
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
        ("code", code),
        ("redirect_uri", config.redirect_uri.as_str()),
    ];

    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub token exchange failed: {}", resp.status());
    }

    let token_resp: GitHubTokenResponse = resp.json().await?;
    Ok(token_resp.access_token)
}

#[derive(Debug, Deserialize)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubEmail {
    pub email: String,
    pub primary: bool,
    pub verified: bool,
}

pub async fn fetch_github_user(token: &str) -> anyhow::Result<GitHubUser> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "KamiSM-OAuth")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned error: {}", resp.status());
    }

    let user: GitHubUser = resp.json().await?;
    Ok(user)
}

pub async fn fetch_github_emails(token: &str) -> anyhow::Result<Vec<GitHubEmail>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/user/emails")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "KamiSM-OAuth")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned error: {}", resp.status());
    }

    let emails: Vec<GitHubEmail> = resp.json().await?;
    Ok(emails)
}

pub fn get_primary_email(user: &GitHubUser, emails: &[GitHubEmail]) -> Option<String> {
    if let Some(ref email) = user.email {
        if !email.is_empty() {
            return Some(email.clone());
        }
    }

    if let Some(primary) = emails.iter().find(|e| e.primary && e.verified) {
        return Some(primary.email.clone());
    }

    if let Some(verified) = emails.iter().find(|e| e.verified) {
        return Some(verified.email.clone());
    }

    if let Some(primary) = emails.iter().find(|e| e.primary) {
        return Some(primary.email.clone());
    }

    emails.first().map(|e| e.email.clone())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthUserInfo {
    pub provider: String,
    pub provider_user_id: String,
    pub username: String,
    pub email: String,
    pub avatar_url: Option<String>,
}
