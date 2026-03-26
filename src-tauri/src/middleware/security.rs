//! 安全响应头中间件
//! 为所有响应注入业界标准安全头

use axum::{
    extract::Request,
    http::{
        header::{self, HeaderName, HeaderValue},
        Method,
    },
    middleware::Next,
    response::Response,
};

/// 注入安全响应头
pub async fn security_headers(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let mut res = next.run(req).await;
    let headers = res.headers_mut();

    // 防止点击劫持
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );

    // 禁止 MIME 嗅探
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );

    // XSS 保护（现代浏览器 CSP 更可靠，此头兼容旧浏览器）
    headers.insert(
        HeaderName::from_static("x-xss-protection"),
        HeaderValue::from_static("1; mode=block"),
    );

    // Referrer 策略：防止泄露路径信息
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    // 禁止使用 FLoC / 隐私沙盒
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), interest-cohort=()",
        ),
    );

    // HSTS：强制 HTTPS（仅 HTTPS 环境生效，HTTP 忽略）
    headers.insert(
        HeaderName::from_static("strict-transport-security"),
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );

    // Content Security Policy
    // - default-src 'self'：只允许同源资源
    // - script-src：允许同源 + Google Fonts 脚本（字体加载）
    // - style-src：允许同源 + Google Fonts CSS + 内联样式（React 运行时需要）
    // - font-src：允许 Google Fonts
    // - img-src：允许同源、data URI（头像占位）
    // - connect-src：允许同源 API + WebSocket（ws/wss）
    // - frame-ancestors 'none'：等效 X-Frame-Options: DENY
    // - form-action 'self'：防止表单提交到外部
    // - base-uri 'self'：防止 base 标签注入
    if method != Method::OPTIONS {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; \
                 script-src 'self'; \
                 style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
                 font-src 'self' https://fonts.gstatic.com; \
                 img-src 'self' data: https:; \
                 connect-src 'self' ws: wss:; \
                 frame-ancestors 'none'; \
                 form-action 'self'; \
                 base-uri 'self'; \
                 object-src 'none'"
            ),
        );
    }

    res
}

