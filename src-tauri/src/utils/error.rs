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
        // ⚠️ `Database` / `Internal` 这两个变体**不能**把原文放进响应体。
        //
        // `sqlx::Error` 的 Display 是**数据库原文** —— 撞唯一约束时会带出
        // `duplicate key value violates unique constraint "cards_code_hash_key"`，
        // 也就是表名 + 列名 + 约束名；`anyhow::Error` 也常常把底层错误一路包上来。
        //
        // 原先这里是 `format!("数据库错误: {}", e)` / `format!("服务器内部错误: {}", e)`。
        // 危险之处在于：**这个类型目前一个调用点都没有**（所以上面挂着
        // `#[allow(dead_code)]`），等于没人踩过 —— 但一旦有人开始 `return AppResult<T>`，
        // 泄漏就是**自动的**，而且因为 dead_code 被 allow 掉，编译器连个警告都不会给。
        //
        // 把它修成安全形状，而不是等出事再改：原文进日志，响应体只回统一文案。
        let (status, message) = match &self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Database(e) => {
                tracing::error!("AppError::Database（原文只进日志）: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "服务器繁忙，请稍后重试".to_string(),
                )
            }
            AppError::Internal(e) => {
                tracing::error!("AppError::Internal（原文只进日志）: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "服务器繁忙，请稍后重试".to_string(),
                )
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// `AppError::Database` 的响应体**不能**带 sqlx 原文。
    ///
    /// 为什么必须用单测盯着它：这个枚举**没有任何调用点**（所以挂着
    /// `#[allow(dead_code)]`），也就没有任何可达的 HTTP 路径 ——
    /// 端到端脚本根本打不到它。但它的 `IntoResponse` 一旦带原文，
    /// 未来任何人开始 `return AppResult<T>` 就自动开始泄漏，且没有编译警告。
    /// 这条断言是唯一能把这个形状钉住的东西。
    #[tokio::test]
    async fn database_variant_does_not_leak_sqlx_detail() {
        let resp = AppError::Database(sqlx::Error::RowNotFound).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let s = body_of(resp).await;
        assert!(
            !s.contains("rows"),
            "响应体泄漏了 sqlx 原文（应为统一文案）: {}",
            s
        );
        assert!(s.contains("服务器繁忙"), "响应体应是统一文案: {}", s);
    }

    /// 同上，`Internal` 变体常把底层错误一路包上来，也不能外泄。
    #[tokio::test]
    async fn internal_variant_does_not_leak_wrapped_detail() {
        let marker = "B11_LEAK_MARKER";
        let resp = AppError::Internal(anyhow::anyhow!(marker)).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let s = body_of(resp).await;
        assert!(!s.contains(marker), "响应体泄漏了 anyhow 原文: {}", s);
        assert!(s.contains("服务器繁忙"), "响应体应是统一文案: {}", s);
    }

    /// 反过来：业务类变体**必须**如实回给用户 —— 不能为了防泄漏把话术也吞掉。
    /// （这条是上一条的对照组：证明上面两条不是因为「所有 message 都变统一文案」而通过。）
    #[tokio::test]
    async fn business_variants_still_return_their_message() {
        let resp = AppError::BadRequest("卡密格式不对".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let s = body_of(resp).await;
        assert!(s.contains("卡密格式不对"), "业务文案不该被吞掉: {}", s);
    }
}

