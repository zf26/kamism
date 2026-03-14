use anyhow::Result;
use lettre::{
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

#[derive(Clone, Debug)]
pub struct MailerConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub from_name: String,
    pub from_email: String,
}

impl MailerConfig {
    pub fn from_env() -> Self {
        Self {
            smtp_host: std::env::var("SMTP_HOST").unwrap_or_else(|_| "smtp.gmail.com".to_string()),
            smtp_port: std::env::var("SMTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(465),
            smtp_user: std::env::var("SMTP_USER").unwrap_or_default(),
            smtp_pass: std::env::var("SMTP_PASS").unwrap_or_default(),
            from_name: std::env::var("SMTP_FROM_NAME").unwrap_or_else(|_| "KamiSM".to_string()),
            from_email: std::env::var("SMTP_FROM_EMAIL").unwrap_or_default(),
        }
    }
}

fn build_verify_email(code: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>KamiSM 邮箱验证</title>
</head>
<body style="margin:0;padding:0;background:#06060a;font-family:'Helvetica Neue',Arial,sans-serif;">
  <table width="100%" cellpadding="0" cellspacing="0" style="background:#06060a;padding:40px 0;">
    <tr><td align="center">
      <table width="520" cellpadding="0" cellspacing="0" style="max-width:520px;width:100%;">

        <!-- Header -->
        <tr><td style="padding:0 0 28px 0;text-align:center;">
          <table cellpadding="0" cellspacing="0" style="margin:0 auto;">
            <tr>
              <td style="background:linear-gradient(135deg,#7c6af7,#a78bfa);border-radius:12px;width:44px;height:44px;text-align:center;vertical-align:middle;">
                <span style="font-size:22px;line-height:44px;">⚡</span>
              </td>
              <td style="padding-left:10px;vertical-align:middle;">
                <span style="font-size:22px;font-weight:800;color:#e8e8f0;letter-spacing:-0.5px;">KamiSM</span>
              </td>
            </tr>
          </table>
        </td></tr>

        <!-- Card -->
        <tr><td style="background:#111118;border:1px solid #1e1e2e;border-radius:16px;padding:40px 36px;">

          <!-- Title -->
          <table width="100%" cellpadding="0" cellspacing="0">
            <tr><td style="padding-bottom:8px;">
              <h1 style="margin:0;font-size:20px;font-weight:800;color:#e8e8f0;letter-spacing:-0.3px;">验证你的邮箱</h1>
            </td></tr>
            <tr><td style="padding-bottom:32px;">
              <p style="margin:0;font-size:14px;color:#888899;line-height:1.6;">
                你正在注册 <strong style="color:#7c6af7;">KamiSM</strong> 商户账号，请使用以下验证码完成验证。<br />
                如果这不是你的操作，请忽略本邮件。
              </p>
            </td></tr>
          </table>

          <!-- Code block -->
          <table width="100%" cellpadding="0" cellspacing="0">
            <tr><td>
              <div style="background:#0a0a0f;border:1px solid #2a2a3e;border-radius:12px;padding:28px;text-align:center;margin-bottom:28px;position:relative;">
                <p style="margin:0 0 8px 0;font-size:11px;font-weight:700;letter-spacing:1.5px;text-transform:uppercase;color:#55556a;">验证码</p>
                <p style="margin:0;font-size:42px;font-weight:800;letter-spacing:16px;color:#7c6af7;font-family:'Courier New',Courier,monospace;text-indent:16px;">{code}</p>
              </div>
            </td></tr>
          </table>

          <!-- Info -->
          <table width="100%" cellpadding="0" cellspacing="0">
            <tr><td style="background:#16161f;border-radius:8px;padding:16px 20px;">
              <table width="100%" cellpadding="0" cellspacing="0">
                <tr>
                  <td style="font-size:13px;color:#888899;">⏱ 有效时间</td>
                  <td style="font-size:13px;color:#e8e8f0;text-align:right;font-weight:600;">10 分钟</td>
                </tr>
                <tr><td colspan="2" style="padding:6px 0;"><hr style="border:none;border-top:1px solid #1e1e2e;margin:0;" /></td></tr>
                <tr>
                  <td style="font-size:13px;color:#888899;">🔒 安全提示</td>
                  <td style="font-size:13px;color:#e8e8f0;text-align:right;font-weight:600;">请勿泄露给他人</td>
                </tr>
              </table>
            </td></tr>
          </table>

        </td></tr>

        <!-- Footer -->
        <tr><td style="padding:24px 0 0 0;text-align:center;">
          <p style="margin:0;font-size:12px;color:#55556a;line-height:1.7;">
            此邮件由 KamiSM 自动发送，请勿直接回复。<br />
            &copy; 2024 KamiSM. All rights reserved.
          </p>
        </td></tr>

      </table>
    </td></tr>
  </table>
</body>
</html>"#,
        code = code
    )
}

pub async fn send_verify_code(config: &MailerConfig, to_email: &str, code: &str) -> Result<()> {
    if config.smtp_user.is_empty() || config.smtp_pass.is_empty() {
        // 开发模式：不发邮件，直接打印到控制台
        tracing::warn!("[开发模式] 验证码邮件 → {}: {}", to_email, code);
        return Ok(());
    }

    let from = format!("{} <{}>", config.from_name, config.from_email);
    let html_body = build_verify_email(code);

    let email = Message::builder()
        .from(from.parse()?)
        .to(to_email.parse()?)
        .subject(format!("【KamiSM】验证码 {} — 10分钟内有效", code))
        .header(ContentType::TEXT_HTML)
        .body(html_body)?;

    let creds = Credentials::new(config.smtp_user.clone(), config.smtp_pass.clone());

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)?
        .port(config.smtp_port)
        .credentials(creds)
        .build();

    mailer.send(email).await?;
    tracing::info!("验证码邮件已发送至: {}", to_email);
    Ok(())
}
