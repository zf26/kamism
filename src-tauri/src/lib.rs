pub mod db;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod utils;
use std::io::Write;
mod workers;

use dotenvy::dotenv;
use std::env;
use std::sync::Arc;
use axum::http::Method;
use tower_http::cors::{Any, CorsLayer};
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::catch_panic::CatchPanicLayer;
use axum::middleware as axum_middleware;
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

    // 捕获所有 panic，打印到 stderr 确保能看到
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let _ = writeln!(std::io::stderr(), "[PANIC] {} at {}", msg, location);
        let _ = std::io::stderr().flush();
    }));

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost/kamism".to_string());
    let jwt_secret = env::var("JWT_SECRET")
        .expect("JWT_SECRET 环境变量未设置 — 请设置一个随机密钥，例如：openssl rand -hex 32");
    let redis_url = env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let amqp_url = env::var("AMQP_URL")
        .unwrap_or_else(|_| "amqp://guest:guest@localhost:5672/%2f".to_string());
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9527);

    tracing::info!("正在连接数据库...");
    let pool = db::create_pool(&database_url).await
        .map_err(|e| anyhow::anyhow!("数据库连接失败: {}", e))?;
    tracing::info!("数据库连接成功");

    db::run_migrations(&pool).await
        .map_err(|e| anyhow::anyhow!("数据库迁移失败: {}", e))?;
    tracing::info!("数据库迁移完成");

    tracing::info!("正在连接 Redis...");
    let redis_client = redis::Client::open(redis_url.as_str())
        .map_err(|e| anyhow::anyhow!("Redis URL 无效: {} (REDIS_URL={})", e, redis_url))?;
    let redis_conn = redis::aio::ConnectionManager::new(redis_client).await
        .map_err(|e| anyhow::anyhow!("Redis 连接失败: {}", e))?;
    tracing::info!("Redis 连接成功");

    tracing::info!("正在连接 RabbitMQ...");
    let mq_channel = utils::mq::connect(&amqp_url).await
        .map_err(|e| anyhow::anyhow!("RabbitMQ 连接失败: {} (AMQP_URL={})", e, amqp_url))?;
    let mq_channel = Arc::new(mq_channel);
    tracing::info!("RabbitMQ 连接成功");

    tracing::info!("正在初始化 KMS...");
    let kms = utils::kms::KmsManager::new()
        .map_err(|e| anyhow::anyhow!("KMS 初始化失败: {}", e))?;
    // 禁止「拿着新密钥去读旧数据」：库里有加密数据时必须用原来那把密钥启动
    ensure_master_key_matches_data(&pool, &kms).await?;

    // 查找哈希的 pepper：由主密钥做**域分隔**派生（HMAC-SHA256(master_key, label)，
    // 与 derive_dek 的 `SHA256(master_key || key_id)` 构造不同，不会撞）。
    // ⚠️ 必须在任何 generate_hash 调用之前注入，否则会 panic。
    db::encrypted_fields::init_lookup_pepper(kms.derive_lookup_pepper());
    tracing::info!(
        "查找哈希算法: HMAC-SHA256(pepper, value)，pepper 指纹 {}（不打印 pepper 本身）",
        db::encrypted_fields::lookup_pepper_fingerprint()
    );
    // 禁止「算法换了、存量行还是旧哈希」：那会让所有按哈希查行的路径静默查不到行
    ensure_lookup_hash_version(&pool).await?;

    let encryptor = Arc::new(utils::kms::Encryptor::new(kms));
    tracing::info!("KMS 初始化成功");

    // 客户端 IP 策略：决定限流与 IP 黑名单到底认哪个地址（默认不信任任何代理）
    let trusted_proxies = utils::client_ip::TrustedProxies::from_env();
    tracing::info!(
        "客户端 IP 策略: {}（TRUSTED_PROXIES={:?}）",
        trusted_proxies.describe(),
        env::var("TRUSTED_PROXIES").unwrap_or_default()
    );

    // 建不出管理员就拒绝启动（理由见 init_admin 的注释）—— 这里**不能**吞掉返回值
    init_admin(&pool).await?;
    let ws_registry = crate::utils::ws::WsRegistry::new();
    let oauth_config_cache = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let payment_config_cache = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let app_url = env::var("VITE_API_URL")
        .unwrap_or_else(|_| "http://localhost:9527".to_string());
    let state = AppState {
        pool: pool.clone(),
        jwt_secret: jwt_secret.clone(),
        mailer: crate::utils::mailer::MailerConfig::from_env(),
        redis: redis_conn.clone(),
        mq_channel: mq_channel.clone(),
        encryptor: encryptor.clone(),
        ws_registry: ws_registry.clone(),
        oauth_config_cache,
        payment_config_cache,
        app_url,
        trusted_proxies,
    };

    // ── 启动降级 / 升级消费者：必须包一层「退避重启」──
    //
    // 两个 worker 的消费循环在 MQ 断连（`consumer.next()` 返回 Err）或消费者创建失败时会
    // **直接 return**。如果只是裸的 `tokio::spawn(run_xxx_worker(...))`，这次 task 结束之后
    // 就再没有任何东西把它拉起来 —— 而扫描器用的是**另一个 channel**，会继续每 60 秒
    // 照常投递、照常打「N 个到期商户已投递」。结果是：消息堆在队列里无人消费，
    // 日志看起来完全正常，所有到期商户永久保持 pro，平台持续漏收。
    //
    // 所以每个 worker 都放进一个循环，退出后带退避重启。
    // 退避策略：连续快速失败 → 1/2/4/8/16/32/60 秒递增；一旦稳定运行超过 60 秒，
    // 说明上次失败只是抖动，把退避重置回 1 秒，避免长时间故障恢复后重连变慢。
    let worker_pool = pool.clone();
    let worker_channel = (*mq_channel).clone();
    let worker_redis = redis_conn.clone();
    tokio::spawn(async move {
        let mut backoff_secs = 1u64;
        loop {
            let started = std::time::Instant::now();
            workers::downgrade::run_downgrade_worker(
                worker_pool.clone(),
                worker_channel.clone(),
                worker_redis.clone(),
            )
            .await;
            if started.elapsed() >= std::time::Duration::from_secs(60) {
                backoff_secs = 1;
            }
            tracing::error!(
                "降级 Worker 已退出（MQ 连接中断或消费者创建失败），{} 秒后重启",
                backoff_secs
            );
            tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
    });

    // 启动升级恢复消费者（同上：带退避重启的循环）
    let upgrade_pool = pool.clone();
    let upgrade_channel = (*mq_channel).clone();
    let upgrade_redis = redis_conn.clone();
    tokio::spawn(async move {
        let mut backoff_secs = 1u64;
        loop {
            let started = std::time::Instant::now();
            workers::downgrade::run_upgrade_worker(
                upgrade_pool.clone(),
                upgrade_channel.clone(),
                upgrade_redis.clone(),
            )
            .await;
            if started.elapsed() >= std::time::Duration::from_secs(60) {
                backoff_secs = 1;
            }
            tracing::error!(
                "升级 Worker 已退出（MQ 连接中断或消费者创建失败），{} 秒后重启",
                backoff_secs
            );
            tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
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

    // CORS：生产环境从环境变量读取允许的 origin，开发环境允许 Any
    let allowed_origin = env::var("ALLOWED_ORIGIN").unwrap_or_default();
    let cors = if allowed_origin.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE, Method::OPTIONS])
            .allow_headers(Any)
    } else {
        use axum::http::HeaderValue;
        let origin = allowed_origin.parse::<HeaderValue>()
            .unwrap_or_else(|_| HeaderValue::from_static("*"));
        CorsLayer::new()
            .allow_origin(origin)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE, Method::OPTIONS])
            .allow_headers(Any)
    };

    routes::health::init_start_time();

    let app = axum::Router::new()
        .merge(routes::health::health_router())
        .merge(routes::auth::auth_router(state.clone()))
        .merge(routes::admin::admin_router_with_state(state.clone()))
        .merge(routes::merchant::merchant_router(state.clone()))
        .merge(routes::apps::apps_router(state.clone()))
        .merge(routes::cards::cards_router(state.clone()))
        .merge(routes::activations::activations_router(state.clone()))
        .merge(routes::public_api::public_api_router(state.clone()))
        .merge(routes::plan_config::plan_config_router(state.clone()))
        .merge(routes::messages::messages_admin_router(state.clone()))
        .merge(routes::messages::messages_merchant_router(state.clone()))
        .merge(routes::messages::messages_ws_router())
        .merge(routes::webhooks::webhooks_router(state.clone()))
        .merge(routes::blacklist::blacklist_router(state.clone()))
        .merge(routes::agent::agent_router(state.clone()))
        .merge(routes::payments::payments_router(state.clone()))
        .merge(routes::oauth::oauth_router(state.clone()))
        .merge(routes::oauth_admin::oauth_admin_router(state.clone()))
        .merge(routes::payment_admin::payment_admin_router(state.clone()))
        .merge(routes::subscription_plan::subscription_plan_router(state.clone()))
        .layer(axum_middleware::from_fn(middleware::security::security_headers))
        // 响应压缩：gzip / brotli，自动根据客户端 Accept-Encoding 协商
        // 对 JSON 响应压缩率通常 60-80%，显著降低带宽占用和客户端解析时间
        .layer(CompressionLayer::new())
        // 请求体大小限制：保护上传最大 ~100MB，通用 API 2MB
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024)) // 100MB
        .layer(cors)
        // 🛡️ 兜底：handler（或任何内层中间件）panic 时回 500，而不是**掐断连接**。
        //
        // 为什么必须有：axum 默认**不装** CatchPanicLayer，一次 panic 会让这条连接的
        // task 直接结束 —— 客户端拿到的是**空回复**（curl 的 http_code=000），
        // 连个 5xx 都没有，前后端都只能猜「是不是网络断了」。
        // 第十二批实测到的三个可达 panic 造成的正是这个现象（见 utils::mask 与
        // routes::oauth::redirect_to 的注释）。
        //
        // ⚠️ 这一层**不能替代**修 panic 本身：它是最后一道网，不是第一道。
        // 局部修复（`utils::mask::*`、`redirect_to`、`is_unique_violation`）负责让
        // 已知 panic 不再发生；这一层负责让**将来任何**漏网 panic 的对外表现变成
        // 「500 + 可读文案」而不是「连接没了」。
        //
        // 放在最外层（`.layer()` 是后加的包在外层），这样内层中间件里的 panic 也能兜住。
        .layer(CatchPanicLayer::custom(
            |err: Box<dyn std::any::Any + Send + 'static>| {
                let detail = if let Some(s) = err.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = err.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "(非字符串 panic payload)".to_string()
                };
                // 与 panic hook 的分工：hook 往 stderr 打「[PANIC] msg at file:line:col」，
                // 这里补上「这次请求的对外结果」。两条要一起看才完整 ——
                // 所以这里刻意把 hook 指出来，免得排查的人只看到其中一条。
                tracing::error!(
                    "handler panic 已被兜住，本次请求回 500（具体位置见同一次的 [PANIC] 行）: {}",
                    detail
                );

                let mut resp = axum::response::Response::new(axum::body::Body::from(
                    r#"{"success":false,"message":"服务器繁忙，请稍后重试"}"#,
                ));
                *resp.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("application/json; charset=utf-8"),
                );
                resp
            },
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await
        .map_err(|e| anyhow::anyhow!("监听端口 {} 失败: {}", port, e))?;
    tracing::info!("KamiSM 服务器已启动，监听端口: {}", port);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// 扫描到期商户，将商户 ID 投递到降级队列。
///
/// **去重不发生在这一层。** 本轮扫描可能与上一轮、或另一个实例重叠，重复投递由下游兜住：
/// worker 侧的 Redis 分布式锁（`kamism:plan_lock:downgrade:*`）挡并发执行，
/// `merchants.updated_at > issued_at` 的乱序校验挡过期消息。
///
/// （此前这里的注释写着「带内存去重」，但函数里从来没有去重逻辑。
/// 错误的注释比没有注释更危险 —— 它会让后来的人以为这层保护存在，从而放心删掉下游的校验。）
async fn scan_and_enqueue(pool: &db::DbPool, channel: &Arc<lapin::Channel>) {
    // ⚠️ 曾用 `.unwrap_or_default()`：查询失败被当成「没有到期商户」，而且因为
    // `published == 0` 时下面不打印日志，**整件事完全没有输出**。
    // 单次失败会在下个 tick 自愈，但**持续失败**（列不存在、连接池耗尽、PG 不可达）
    // 会让所有到期商户永久保持 pro —— 运维在日志里看不到任何线索。
    let expired: Vec<(uuid::Uuid,)> = match sqlx::query_as(
        "SELECT id FROM merchants
         WHERE plan = 'pro'
           AND plan_expires_at IS NOT NULL
           AND plan_expires_at <= NOW()
         ORDER BY plan_expires_at ASC",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(
                "扫描到期商户失败：本轮到期的商户不会被降级，需等下一个周期重试: {}",
                e
            );
            return;
        }
    };

    let mut published = 0u32;
    for (merchant_id,) in &expired {
        if let Err(e) = utils::mq::publish_downgrade(channel, &merchant_id.to_string()).await {
            tracing::error!("发布降级消息失败 {}: {}", merchant_id, e);
        } else {
            published += 1;
        }
    }
    if published > 0 {
        tracing::info!("降级扫描完成: {} 个到期商户已投递", published);
    }
}

/// 启动前置检查：禁止「拿着新密钥去读旧数据」
///
/// 没有配置 MASTER_KEY 时进程会临时生成一把密钥。如果数据库里已经有加密数据
/// （商户的 api_key / 邮箱、卡密的 code、激活记录的 device_id 都是 AES-256-GCM
/// 加密存储），说明存量数据用的是另一把密钥 —— 继续启动只会让这些字段全部解不开，
/// 而且表现为「接口 200 但字段读不出来」这种最难查的形态。
/// 所以这里直接拒绝启动：把安静的数据损坏，换成启动时的大声报错。
async fn ensure_master_key_matches_data(
    pool: &db::DbPool,
    kms: &utils::kms::KmsManager,
) -> anyhow::Result<()> {
    if !kms.is_auto_generated() {
        return Ok(());
    }

    let (merchants,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM merchants")
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow::anyhow!("无法确认存量数据规模（MASTER_KEY 校验失败）: {}", e))?;
    let (cards,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cards")
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow::anyhow!("无法确认存量数据规模（MASTER_KEY 校验失败）: {}", e))?;

    if merchants > 0 || cards > 0 {
        anyhow::bail!(
            "拒绝启动：数据库已有 {} 个商户、{} 张卡密（敏感字段为加密存储），\
但当前进程没有可用的 MASTER_KEY（未设置，已临时生成一把新密钥）。\
继续启动会让这些字段永久无法解密（api_key、邮箱、卡密、设备 ID 全部读不出来）。\
请把原来的 MASTER_KEY 配到环境变量里再重启；若是全新部署，请先固定一个 MASTER_KEY 再写入数据。\
生成方式：openssl rand -hex 32",
            merchants,
            cards
        );
    }

    Ok(())
}

/// 启动前置检查：禁止「查询哈希的算法换了、存量行还是旧哈希」
///
/// 查找哈希列（`merchants.api_key_hash` / `email_hash`、`cards.code_hash`、
/// `activations.device_id_hash`）是用来做 `WHERE xxx_hash = $1` 精确匹配的。
/// 这批把算法从裸 SHA-256（v1）换成了 `HMAC-SHA256(pepper, value)`（v2），
/// **算法一换，存量行就一个都查不到** —— 表现为「用户拿着正确的卡密被告知不存在」
/// 「登录说邮箱没注册过」，而库连着、接口 200、日志干净。
///
/// 所以这里用 `encryption_keys` 里的一行标记来判断回填跑没跑过
/// （判定逻辑见 `db::lookup_hash::startup_decision`，那边有单测；这里只负责执行）：
///   - 标记已是 v2        → 放行
///   - 库里一行数据都没有  → 全新库，写入 v2 标记后放行
///   - 有数据但没有 v2 标记 → **拒绝启动**，并告诉你怎么修
///
/// 取舍与 `ensure_master_key_matches_data` 一致：**把静默的查询失效，
/// 换成启动时的一条明确指令。**
async fn ensure_lookup_hash_version(pool: &db::DbPool) -> anyhow::Result<()> {
    use db::lookup_hash::{self, StartupDecision, V2_HMAC_SHA256};

    let marker = lookup_hash::read_marker(pool).await?;
    let counts = lookup_hash::row_counts(pool).await?;

    match lookup_hash::startup_decision(marker, &counts) {
        StartupDecision::Ok => {
            tracing::info!(
                "查找哈希版本: v{V2_HMAC_SHA256}（回填已完成，当前 {}）",
                counts.describe()
            );
            Ok(())
        }
        StartupDecision::MarkFresh => {
            lookup_hash::write_marker(pool, V2_HMAC_SHA256).await?;
            tracing::info!(
                "查找哈希版本: 全新库（{}），已标记为 v{V2_HMAC_SHA256}",
                counts.describe()
            );
            Ok(())
        }
        StartupDecision::NeedsRehash => anyhow::bail!(
            "拒绝启动：库里有数据（{}），但没有「查找哈希已回填到 v{}」的标记。\n\
             这批把按哈希查行的算法从裸 SHA-256 换成了 HMAC-SHA256(pepper, value)，\
             存量行的哈希还是旧算法 —— 继续启动会让**所有**按哈希查行的路径都查不到行：\n\
             · 用户拿着正确的卡密被告知「卡密不存在」\n\
             · 登录时说「邮箱没注册过」\n\
             · 解绑/删除路径删不到本该删的行\n\
             而这些都不会报错。修复只需一条命令（可反复跑，幂等）：\n\
             \n\
             \x20   cargo run --bin rehash_lookup_columns            # 先看要动多少行\n\
             \x20   cargo run --bin rehash_lookup_columns -- --apply  # 确认后写库\n\
             \n\
             注意 `device_blacklist.device_id_hash` 无法回填（那张表没有明文可回收），\
             运行时已按新旧两种哈希同时匹配，不需要人工处理。",
            counts.describe(),
            V2_HMAC_SHA256
        ),
    }
}

/// 确保库里至少有一个管理员账号。
///
/// ⚠️ 这个函数曾经有**三层静默叠加**，任何一层出问题都会造成
/// 「没有任何管理员、登录不了、而日志说一切正常」的死局：
///
/// ```ignore
/// let exists = sqlx::query_as("SELECT id::text FROM admins LIMIT 1")
///     .fetch_optional(pool).await
///     .unwrap_or(None);          // ① 库故障被压成「还没有管理员」→ 继续去建
/// if exists.is_some() { return; }
/// let _ = sqlx::query("INSERT INTO admins ...")
///     .execute(pool).await;      // ② 建账号失败被丢弃
/// tracing::info!("初始管理员账号已创建: {}", admin_email);  // ③ 无条件声称成功
/// ```
///
/// 其中 ③ 最要命：它把「没建成」也报成了「建成」。运维看到这行日志就会
/// 停止排查权限问题，转头去查密码错在哪 —— 而库里其实一个管理员都没有。
///
/// 现在的取舍与 `ensure_master_key_matches_data` 一致：**拒绝启动**。
/// 理由：没有管理员账号 = 后台完全进不去，属于「不可用」状态；
/// 带着它继续跑只会让问题在更靠后的地方以更难查的形式出现。
/// 启动失败会打印明确的错误，编排器重启即可重试（真·全新建库时这是幂等的）。
async fn init_admin(pool: &db::DbPool) -> anyhow::Result<()> {
    // 「查不到」和「查不了」必须分开：前者是全新部署（继续去建），
    // 后者是数据库故障（拒绝启动）。这正是 `unwrap_or(None)` 抹掉的那条分支。
    let exists: Option<(String,)> = sqlx::query_as("SELECT id::text FROM admins LIMIT 1")
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "检查管理员账号失败（这是数据库错误，不是「还没有管理员」）: {}",
                e
            )
        })?;

    if exists.is_some() {
        return Ok(());
    }

    let admin_email = env::var("ADMIN_EMAIL").unwrap_or_else(|_| "admin@kamism.com".to_string());
    let admin_password = env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "Admin@123456".to_string());
    let password_hash = bcrypt::hash(&admin_password, bcrypt::DEFAULT_COST)
        .map_err(|e| anyhow::anyhow!("初始管理员密码哈希失败: {}", e))?;

    sqlx::query("INSERT INTO admins (username, email, password_hash) VALUES ($1, $2, $3)")
        .bind("admin")
        .bind(&admin_email)
        .bind(&password_hash)
        .execute(pool)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "初始管理员账号创建失败（库里当前没有任何管理员，继续启动会导致无法登录，因此拒绝启动）: \
                 email={} err={}。请修复数据库后重启；或设置 ADMIN_EMAIL / ADMIN_PASSWORD 后重试。",
                admin_email,
                e
            )
        })?;

    tracing::info!("初始管理员账号已创建: {}", admin_email);
    Ok(())
}
