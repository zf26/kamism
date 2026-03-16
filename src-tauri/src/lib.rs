pub mod db;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod utils;
mod workers;

use dotenvy::dotenv;
use std::env;
use std::sync::Arc;
use axum::http::Method;
use tower_http::cors::{Any, CorsLayer};
use crate::middleware::auth::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// 返回配置的 API 服务器地址（供前端使用）
/// API_URL 在编译时通过环境变量写死进二进制，打包后不依赖 .env 文件
#[cfg(feature = "desktop")]
#[tauri::command]
fn get_api_url() -> String {
    // 编译时确定的服务器地址，优先级：编译时 API_URL 环境变量 > 默认值
    option_env!("API_URL").unwrap_or("http://localhost:9527").to_string()
}

/// Tauri 桌面客户端入口（仅 desktop feature 启用时编译）
#[cfg(feature = "desktop")]
pub fn run() {
    let _ = dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_api_url])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用失败");
}

/// 独立服务器入口（供 server/ crate 调用）
pub async fn start_server() -> anyhow::Result<()> {
    let _ = dotenv();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost/kamism".to_string());
    let jwt_secret = env::var("JWT_SECRET")
        .unwrap_or_else(|_| "kamism-super-secret-key-change-in-production".to_string());
    let redis_url = env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let amqp_url = env::var("AMQP_URL")
        .unwrap_or_else(|_| "amqp://guest:guest@localhost:5672/%2f".to_string());
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9527);

    tracing::info!("正在连接数据库...");
    let pool = db::create_pool(&database_url).await?;
    tracing::info!("数据库连接成功");

    db::run_migrations(&pool).await?;
    tracing::info!("数据库迁移完成");

    tracing::info!("正在连接 Redis...");
    let redis_client = redis::Client::open(redis_url.as_str())?;
    let redis_conn = redis::aio::ConnectionManager::new(redis_client).await?;
    tracing::info!("Redis 连接成功");

    tracing::info!("正在连接 RabbitMQ...");
    let mq_channel = utils::mq::connect(&amqp_url).await?;
    let mq_channel = Arc::new(mq_channel);
    tracing::info!("RabbitMQ 连接成功");

    tracing::info!("正在初始化 KMS...");
    let kms = utils::kms::KmsManager::new()?;
    let encryptor = Arc::new(utils::kms::Encryptor::new(kms));
    tracing::info!("KMS 初始化成功");

    init_admin(&pool).await;
    let state = AppState {
        pool: pool.clone(),
        jwt_secret: jwt_secret.clone(),
        mailer: crate::utils::mailer::MailerConfig::from_env(),
        redis: redis_conn.clone(),
        mq_channel: mq_channel.clone(),
        encryptor: encryptor.clone(),
    };

    // 启动降级消费者（独立 task，传入独立 Redis 连接）
    let worker_pool = pool.clone();
    let worker_channel = (*mq_channel).clone();
    let worker_redis = redis_conn.clone();
    tokio::spawn(async move {
        workers::downgrade::run_downgrade_worker(worker_pool, worker_channel, worker_redis).await;
    });

    // 启动升级恢复消费者（独立 task，传入独立 Redis 连接）
    let upgrade_pool = pool.clone();
    let upgrade_channel = (*mq_channel).clone();
    let upgrade_redis = redis_conn.clone();
    tokio::spawn(async move {
        workers::downgrade::run_upgrade_worker(upgrade_pool, upgrade_channel, upgrade_redis).await;
    });

    // 启动定时扫描任务：每 60 秒扫描一次到期商户，发布降级消息
    let scanner_pool = pool.clone();
    let scanner_channel = mq_channel.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            scan_and_enqueue(&scanner_pool, &scanner_channel).await;
        }
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);

    let app = axum::Router::new()
        .merge(routes::auth::auth_router(state.clone()))
        .merge(routes::admin::admin_router_with_state(state.clone()))
        .merge(routes::merchant::merchant_router(state.clone()))
        .merge(routes::apps::apps_router(state.clone()))
        .merge(routes::cards::cards_router(state.clone()))
        .merge(routes::activations::activations_router(state.clone()))
        .merge(routes::public_api::public_api_router(state.clone()))
        .merge(routes::plan_config::plan_config_router(state.clone()))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("KamiSM 服务器已启动，监听端口: {}", port);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// 扫描到期商户，将商户 ID 投递到降级队列
async fn scan_and_enqueue(pool: &db::DbPool, channel: &Arc<lapin::Channel>) {
    let expired: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM merchants
         WHERE plan = 'pro'
           AND plan_expires_at IS NOT NULL
           AND plan_expires_at <= NOW()",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (merchant_id,) in expired {
        if let Err(e) = utils::mq::publish_downgrade(channel, &merchant_id.to_string()).await {
            tracing::error!("发布降级消息失败 {}: {}", merchant_id, e);
        } else {
            tracing::info!("已发布降级消息: 商户 {}", merchant_id);
        }
    }
}

async fn init_admin(pool: &db::DbPool) {
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id::text FROM admins LIMIT 1")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if exists.is_some() {
        return;
    }

    let admin_email = env::var("ADMIN_EMAIL").unwrap_or_else(|_| "admin@kamism.com".to_string());
    let admin_password = env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "Admin@123456".to_string());
    let password_hash = bcrypt::hash(&admin_password, bcrypt::DEFAULT_COST).unwrap();

    let _ = sqlx::query(
        "INSERT INTO admins (username, email, password_hash) VALUES ($1, $2, $3)",
    )
    .bind("admin")
    .bind(&admin_email)
    .bind(&password_hash)
    .execute(pool)
    .await;

    tracing::info!("初始管理员账号已创建: {}", admin_email);
}
