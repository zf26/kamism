use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Admin {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 令牌版本号（吊销机制，见 `migrations/009_token_version.sql`）。
    ///
    /// `#[serde(skip_serializing)]` 的理由：这个字段只用于服务端校验，
    /// 不该出现在任何返回给前端的 admin 序列化结果里 —— 它是内部计数器，
    /// 泄漏出去没有直接危害，但能让攻击者判断「我手里这张 token 是不是
    /// 刚被吊销的那一批」。
    ///
    /// 登录响应是手写的 `json!`（不经过这个结构体的 Serialize），
    /// 所以加这个属性不影响登录返回值 —— 本来也不会把整个 Admin 返给前端。
    #[serde(skip_serializing)]
    pub token_version: i32,
}

