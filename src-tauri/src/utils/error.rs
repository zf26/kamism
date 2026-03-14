use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error("数据库错误: {0}")]
    Database(#[from] sqlx::Error),

    #[error("未找到: {0}")]
    NotFound(String),

    #[error("未授权: {0}")]
    Unauthorized(String),

    #[error("参数错误: {0}")]
    BadRequest(String),

    #[error("内部错误: {0}")]
    Internal(#[from] anyhow::Error),

    #[error("卡密错误: {0}")]
    Card(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Database(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("数据库错误: {}", e),
            ),
            AppError::Internal(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("服务器内部错误: {}", e),
            ),
            AppError::Card(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
        };

        let body = Json(json!({
            "success": false,
            "message": message
        }));

        (status, body).into_response()
    }
}

#[allow(dead_code)]
pub type AppResult<T> = Result<T, AppError>;

