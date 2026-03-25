use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 消息类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    Notice,
    Message,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::Notice => write!(f, "notice"),
            MessageType::Message => write!(f, "message"),
        }
    }
}

/// 消息收件人范围
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    All,
    Single,
}

impl std::fmt::Display for TargetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetType::All => write!(f, "all"),
            TargetType::Single => write!(f, "single"),
        }
    }
}

/// 数据库原始行
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    #[sqlx(rename = "type")]
    pub msg_type: String,
    pub title: String,
    pub content: String,
    pub sender_id: Uuid,
    pub target_type: String,
    pub target_id: Option<Uuid>,
    pub pinned: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 管理员视图（含已读人数）
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageAdminView {
    pub id: Uuid,
    pub msg_type: String,
    pub title: String,
    pub content: String,
    pub sender_id: Uuid,
    pub target_type: String,
    pub target_id: Option<Uuid>,
    pub pinned: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub read_count: i64,
    pub created_at: DateTime<Utc>,
}

/// 商户视图（含是否已读）
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageMerchantView {
    pub id: Uuid,
    pub msg_type: String,
    pub title: String,
    pub content: String,
    pub target_type: String,
    pub pinned: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

/// 已读记录
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MessageRead {
    pub id: Uuid,
    pub message_id: Uuid,
    pub merchant_id: Uuid,
    pub read_at: DateTime<Utc>,
}

