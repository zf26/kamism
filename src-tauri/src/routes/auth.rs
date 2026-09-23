use crate::{
    db::encrypted_fields::EncryptedFieldsOps,
    middleware::{
        auth::AppState,
        rate_limit::login_rate_limit,
    },
    models::merchant::Merchant,
    utils::{
        card_gen::generate_api_key,
        db_guard,
        jwt::{generate_token, generate_refresh_token, verify_refresh_token},
        mailer::send_verify_code,
        redis_guard,
    },
};
use axum::{
    extract::State,
    middleware,
    routing::post,
    Json, Router,
};
use bcrypt::{hash, verify};
use rand::Rng;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub code: String,
}

#[derive(Deserialize)]
pub struct SendCodeRequest {
    pub email: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub email: String,
    pub code: String,
    pub new_password: String,
}

pub fn auth_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/auth/send-code", post(send_code))
        .route("/auth/register", post(register))
        .route("/auth/refresh", post(refresh_token))
        .route("/auth/send-reset-code", post(send_reset_code))
        .route("/auth/reset-password", post(reset_password))
        .route(
            "/auth/login",
            post(login).route_layer(
                middleware::from_fn_with_state(state, login_rate_limit)
            ),
        )
}

/// 发送注册验证码
async fn send_code(
    State(state): State<AppState>,
    Json(body): Json<SendCodeRequest>,
) -> Json<Value> {
    if !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }

    // 检查邮箱是否已注册（使用哈希索引查询）
    //
    // ⚠️ 这里曾用 `.unwrap_or(None)`：数据库出错 → `None` → 判定「没注册过」→
    // 给一个**已经注册的邮箱**发注册验证码。用户会一路走到最后一步才失败，
    // 而更早的一层「邮箱已注册」的提示被跳过了（也是轻微的信息泄漏）。
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let exists = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE email_hash = $1 LIMIT 1")
            .bind(&email_hash)
            .fetch_optional(&state.pool),
        "检查邮箱是否已注册（send_code）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };
    if exists.is_some() {
        return Json(json!({"success": false, "message": "该邮箱已注册"}));
    }

    let mut redis = state.redis.clone();
    let cooldown_key = format!("code:cooldown:{}", body.email);

    // 60秒冷却防刷
    let in_cooldown: bool = redis.exists(&cooldown_key).await.unwrap_or(false);
    if in_cooldown {
        return Json(json!({"success": false, "message": "请求过于频繁，请60秒后再试"}));
    }

    // 生成6位数字验证码
    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Uniform::new(0u32, 10))
        .take(6)
        .map(|d| char::from_digit(d, 10).unwrap())
        .collect();

    // 存入 Redis：验证码 10 分钟过期，冷却标记 60 秒过期
    //
    // ⚠️ 两者失败后的处置**不同**，不能改成同一种：
    //   · 验证码是这次操作的**唯一凭据** —— 存不下就绝不能宣称「已发送」。
    //     否则用户拿到一个永远验证不通过的码，最自然的反应是「再发一次」，
    //     而每一次发送同样写不进去 → 现象完全符合「验证码就是收不到」的直觉，
    //     真正的原因（Redis 写失败）一个字都不在响应里。所以走 `required`：
    //     写不进就直接拒绝，**不发信**。
    //   · 冷却标记是**防刷措施**，性质同限流（见 `rate_limit.rs` 里 fail-open 的取舍）：
    //     写不进只丢防刷，不该阻塞一次合法的发送。走 `protection_lost`。
    //
    // 注意写入顺序：**先存码、再发信**。反过来的话，信已经出去了而码没存上，
    // 又是一次「说了做不到」。
    let code_key = format!("code:verify:{}", body.email);
    if redis_guard::required::<()>(
        redis.set_ex(&code_key, &code, 600).await,
        "写入注册验证码",
    )
    .is_err()
    {
        // 文案与下面「读验证码」处的 Redis 故障分支保持一致
        // （register / reset_password 里那句「服务暂时不可用，请稍后重试」）。
        return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
    }
    redis_guard::protection_lost::<()>(
        redis.set_ex(&cooldown_key, "1", 60).await,
        "写入注册冷却标记",
    );

    // 发送邮件
    match send_verify_code(&state.mailer, &body.email, &code).await {
        Ok(_) => Json(json!({"success": true, "message": "验证码已发送，请查收邮件"})),
        Err(e) => {
            tracing::error!("发送验证码邮件失败: {}", e);
            // 发送失败则清除 Redis 记录，允许重试（best_effort：清不掉也只是留个死码，
            // 10 分钟后自然过期，不影响用户重试）
            redis_guard::best_effort::<()>(redis.del(&code_key).await, "清理注册验证码");
            redis_guard::best_effort::<()>(redis.del(&cooldown_key).await, "清理注册冷却标记");
            Json(json!({"success": false, "message": "邮件发送失败，请稍后重试"}))
        }
    }
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Json<Value> {
    if !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }
    if body.email.len() > 254 {
        return Json(json!({"success": false, "message": "邮箱长度超限"}));
    }
    if body.password.len() < 8 {
        return Json(json!({"success": false, "message": "密码至少8位"}));
    }
    if body.password.len() > 128 {
        return Json(json!({"success": false, "message": "密码长度超限"}));
    }
    if body.username.len() < 3 {
        return Json(json!({"success": false, "message": "用户名至少3位"}));
    }
    if body.username.len() > 32 {
        return Json(json!({"success": false, "message": "用户名最长32位"}));
    }
    if !body.username.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Json(json!({"success": false, "message": "用户名只能包含字母、数字、下划线和连字符"}));
    }
    if body.code.len() != 6 || !body.code.chars().all(|c| c.is_ascii_digit()) {
        return Json(json!({"success": false, "message": "验证码格式错误"}));
    }

    let mut redis = state.redis.clone();
    let code_key = format!("code:verify:{}", body.email);

    // 从 Redis 取出验证码
    //
    // ⚠️ 这里曾用 `.unwrap_or(None)`，Redis 故障会被伪装成「验证码无效或已过期」。
    // 这个伪装特别难查：用户会以为验证码过期了，于是一遍遍点「重新发送」——
    // 而每一次发送都会真的写进 Redis（大概率也是失败的），界面上的表现
    // 完全符合「验证码就是收不到/用不了」的直觉。真正的原因（Redis 连不上）
    // 一个字都不在日志里。
    //
    // 注意这一段是**手写**的三分支，道理与 db_guard 一样：`Ok(None)` 和 `Err` 是两回事。
    // （`db_guard::internal_error` 现在也接受 `impl Display`，技术上能用在这儿；
    //  这里没换，是因为本段已经是对的 —— 不泄漏、有日志 ——
    //  换过去只是把文案从「服务暂时不可用」改成「服务器繁忙」，没有安全收益。
    //  要统一的话，该做的是一次文案口径的统一，而不是把 Redis 硬塞进名叫 db 的模块。）
    let stored_code: Option<String> = match redis.get(&code_key).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("读取注册验证码失败（Redis 错误，不是「验证码不存在」）: {}", e);
            return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
        }
    };
    match stored_code {
        None => return Json(json!({"success": false, "message": "验证码无效或已过期"})),
        Some(c) if c != body.code => return Json(json!({"success": false, "message": "验证码错误"})),
        Some(_) => {
            // 验证通过，立即删除（一次性）。
            // 这是**防护**而不是派生态：删不掉的话，这个码在剩余 TTL 内还能被重复使用。
            // 方向是安全的（不该因为删不掉就阻塞注册），所以放行，但必须留痕 ——
            // 否则「同一个码用了两次」将来无从查起。
            redis_guard::protection_lost::<()>(
                redis.del(&code_key).await,
                "消耗注册验证码（一次性保证）",
            );
        }
    }

    // 检查用户名是否已存在
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：数据库出错 → 当成「用户名可用」→ 继续往下注册。
    // 如果 UNIQUE 约束恰好没挡住（比如约束建在别的列上，或这次是全新的值），
    // 注册就会「成功」，重名账号由此产生。即使约束挡住了，
    // 用户看到的也是「注册失败」而不是「服务暂时不可用」。
    let exists = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE username = $1 LIMIT 1")
            .bind(&body.username)
            .fetch_optional(&state.pool),
        "检查用户名是否已存在（register）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if exists.is_some() {
        return Json(json!({"success": false, "message": "用户名已存在"}));
    }

    // 检查邮箱是否已注册（使用哈希索引查询）
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let email_exists = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE email_hash = $1 LIMIT 1")
            .bind(&email_hash)
            .fetch_optional(&state.pool),
        "检查邮箱是否已注册（register）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    if email_exists.is_some() {
        return Json(json!({"success": false, "message": "邮箱已存在"}));
    }

    // cost=10：在安全（抗暴力破解）和性能（登录延迟<300ms）间取得最佳平衡
    // bcrypt DEFAULT_COST=12 在现代硬件上约需 800ms~1200ms，对登录接口过慢
    let password_hash = match hash(&body.password, 10) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    let api_key = generate_api_key();
    let merchant_id = Uuid::new_v4();

    // 生成哈希值
    let api_key_hash = EncryptedFieldsOps::generate_hash(&api_key);
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);

    // ── 加密 + 插商户 + 记加密日志：同一个事务 ─────────────────────────────
    //
    // ⚠️ 改之前是：先 `encrypt_merchant_api_key(&state.pool, ..)` 和
    // `encrypt_merchant_email(&state.pool, ..)`（这两个旧函数内部各自往
    // `encrypted_fields_log` 写一条，用的是池上自开的短事务），**然后**才
    // INSERT 商户。两个问题：
    //
    //   1. 那两条日志在商户行存在之前就已经**提交**了。只要 INSERT 失败，
    //      就留下两条指向「不存在的商户」的孤儿日志。
    //
    //      什么时候会失败？注意上面的 `exists` / `email_exists` 两次查询已经把
    //      「用户名重复」「邮箱重复」挡成友好提示了，所以走到 INSERT 还能失败的
    //      是它们**挡不住**的情况：
    //        · 并发注册同一用户名/邮箱 —— 两个请求都通过预检查，
    //          后落库的那个撞 UNIQUE 约束（预检查天然挡不住竞态）
    //        · 数据库瞬时故障（连接断、超时）
    //      不是高频路径，但形态一样：每发生一次多两条垃圾日志，
    //      而外部只看到一行「注册失败」，没人会想到去查加密日志表。
    //   2. 加密本身不碰数据库（纯 AEAD），拆成两段只为了「先写日志」，
    //      把一次注册变成三条独立提交。
    //
    // 现在：加密在事务里做，日志用 `_tx` 变体写。事务外写日志的公共入口已从
    // `EncryptedFieldsOps` 全部删除（见 `db/encrypted_fields.rs` 的模块说明），
    // 所以这里想写错也不太可能了。
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("注册时开启事务失败: email={} err={}", body.email, e);
            return Json(json!({"success": false, "message": "注册失败，请稍后重试"}));
        }
    };

    // 加密 API Key 和邮箱（日志写进上面这个事务）
    let encrypted_api_key = match EncryptedFieldsOps::encrypt_merchant_api_key_tx(
        &mut *tx,
        &state.encryptor,
        merchant_id,
        &api_key,
    ).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("加密 API Key 失败: {}", e);
            return Json(json!({"success": false, "message": "注册失败"}));
        }
    };

    let encrypted_email = match EncryptedFieldsOps::encrypt_merchant_email_tx(
        &mut *tx,
        &state.encryptor,
        merchant_id,
        &body.email,
    ).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("加密邮箱失败: {}", e);
            return Json(json!({"success": false, "message": "注册失败"}));
        }
    };

    let result = sqlx::query(
        "INSERT INTO merchants (id, username, email_encrypted, email_hash, password_hash, api_key_encrypted, api_key_hash, email_verified) VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE)",
    )
    .bind(merchant_id)
    .bind(&body.username)
    .bind(&encrypted_email)
    .bind(&email_hash)
    .bind(&password_hash)
    .bind(&encrypted_api_key)
    .bind(&api_key_hash)
    .execute(&mut *tx)
    .await;

    match result {
        Ok(_) => {
            // 提交失败必须说出来。上一版这里是 `Ok(_) => 注册成功` ——
            // 把「没提交成功」也报成成功，用户拿着密码去登录会发现账号不存在，
            // 而服务端一个字都没记。
            if let Err(e) = tx.commit().await {
                tracing::error!("提交注册事务失败: email={} err={}", body.email, e);
                return Json(json!({"success": false, "message": "注册失败，请稍后重试"}));
            }
            Json(json!({"success": true, "message": "注册成功，请登录"}))
        }
        // INSERT 失败 → `tx` 在函数返回时析构并回滚，那两条加密日志一起消失
        //
        // ⚠️ 这里是**未登录可达**的端点，曾经回的是 `format!("注册失败: {}", e)` ——
        // sqlx 的 Display 是数据库原文，撞唯一约束时会带出
        // `duplicate key value violates unique constraint "merchants_username_key"`，
        // 也就是把表名/列名/约束名送给了任何人。改走日志。
        Err(e) => db_guard::internal_error("注册商户（INSERT merchants）", e),
    }
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Json<Value> {
    // 先查管理员表
    let admin_result: Result<Option<crate::models::admin::Admin>, _> =
        sqlx::query_as("SELECT * FROM admins WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.pool)
            .await;
    let admin = match admin_result {
        Ok(opt) => opt,
        Err(e) => {
            tracing::error!("查询 admins 表失败: {}", e);
            // 上面一行已经把真实错误记进日志了；响应体**只回统一文案**。
            // 原先是 `format!("服务器内部错误: {}", e)` —— /auth/login 是未登录可达的，
            // 等于把 `SELECT * FROM admins/merchants` 出错时的数据库原文（表名、列名）
            // 直接给了任何人。
            return db_guard::server_busy();
        }
    };
    if let Some(admin) = admin {
        let valid = verify(&body.password, &admin.password_hash).unwrap_or(false);
        if !valid {
            return Json(json!({"success": false, "message": "邮箱或密码错误"}));
        }
        let token = match generate_token(&admin.id, "admin", &admin.email, admin.token_version as i64, &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
        };
        let refresh_token = match generate_refresh_token(&admin.id, "admin", &admin.email, admin.token_version as i64, &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
        };
        return Json(json!({
            "success": true,
            "token": token,
            "refresh_token": refresh_token,
            "role": "admin",
            "user": {
                "id": admin.id,
                "username": admin.username,
                "email": admin.email,
            }
        }));
    }

    // 再查商户表（使用哈希索引查询）
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let merchant_result: Result<Option<Merchant>, _> =
        sqlx::query_as("SELECT * FROM merchants WHERE email_hash = $1")
            .bind(&email_hash)
            .fetch_optional(&state.pool)
            .await;
    let merchant = match merchant_result {
        Ok(opt) => opt,
        Err(e) => {
            tracing::error!("查询 merchants 表失败: {}", e);
            // 上面一行已经把真实错误记进日志了；响应体**只回统一文案**。
            // 原先是 `format!("服务器内部错误: {}", e)` —— /auth/login 是未登录可达的，
            // 等于把 `SELECT * FROM admins/merchants` 出错时的数据库原文（表名、列名）
            // 直接给了任何人。
            return db_guard::server_busy();
        }
    };
    tracing::info!("查询 merchants 表结果: email_hash={:?}, result={:?}", email_hash, merchant);

    let merchant = match merchant {
        Some(m) => m,
        None => return Json(json!({"success": false, "message": "邮箱或密码错误"})),
    };

    if merchant.status == "disabled" {
        return Json(json!({"success": false, "message": "账号已被禁用"}));
    }

    let valid = verify(&body.password, &merchant.password_hash).unwrap_or(false);
    if !valid {
        return Json(json!({"success": false, "message": "邮箱或密码错误"}));
    }

    let token = match generate_token(&merchant.id, "merchant", &merchant.email, merchant.token_version as i64, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };
    let refresh_token = match generate_refresh_token(&merchant.id, "merchant", &merchant.email, merchant.token_version as i64, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };

    // 解密 API Key 和邮箱
    let api_key = match EncryptedFieldsOps::decrypt_merchant_api_key(&state.encryptor, &merchant.api_key) {
        Ok(key) => key,
        Err(e) => {
            tracing::error!("解密 API Key 失败: {}", e);
            return Json(json!({"success": false, "message": "解密失败"}));
        }
    };

    let email = match EncryptedFieldsOps::decrypt_merchant_email(&state.encryptor, &merchant.email) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("解密邮箱失败: {}", e);
            return Json(json!({"success": false, "message": "解密失败"}));
        }
    };

    Json(json!({
        "success": true,
        "token": token,
        "refresh_token": refresh_token,
        "role": "merchant",
        "user": {
            "id": merchant.id,
            "username": merchant.username,
            "email": email,
            "api_key": api_key,
            "status": merchant.status,
            "plan": merchant.plan,
            "plan_expires_at": merchant.plan_expires_at,
            "email_verified": merchant.email_verified,
            "created_at": merchant.created_at
        }
    }))
}

async fn refresh_token(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Json<Value> {
    let claims = match verify_refresh_token(&body.refresh_token, &state.jwt_secret) {
        Ok(c) => c,
        Err(_) => return Json(json!({"success": false, "message": "Refresh Token 无效或已过期，请重新登录"})),
    };

    let user_id = match uuid::Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => return Json(json!({"success": false, "message": "无效用户ID"})),
    };

    // ── ① 吊销校验（必须先做，且必须在「账号是否有效」之前）──────────────
    //
    // 顺序很重要：如果先查「账号是否 active」再查吊销，那对于**被禁用**的商户，
    // 两次查询会给出相同的拒绝结果，看起来没区别 —— 但语义不同：
    // 「已被禁用」和「令牌已被吊销」要给出不同的提示，否则用户不知道该做什么
    // （前者要找管理员，后者重新登录就行）。
    //
    // ⚠️ 这一层不能省。没有它的话，改密码/重置密码**完全不影响**已签发的 refresh：
    // 攻击者拿着旧 refresh 可以一直续下去。而且因为每次 refresh 都会滚动签发
    // 新的 refresh（见下面 ③），受害者在别处重新登录也不会让攻击者掉线 ——
    // 攻击者的链条是**自持**的，理论上能续到天荒地老，不只是 7 天。
    match crate::utils::jwt::check_token_version(
        &state.pool,
        &mut state.redis.clone(),
        &claims.role,
        &user_id,
        claims.ver,
    )
    .await
    {
        crate::utils::jwt::VersionCheck::Valid => {}
        crate::utils::jwt::VersionCheck::Revoked { token_ver, current_ver } => {
            tracing::info!(
                "拒绝已吊销的 refresh token: user_id={} role={} token_ver={} current_ver={}",
                user_id, claims.role, token_ver, current_ver
            );
            return Json(json!({"success": false, "message": "登录状态已失效，请重新登录"}));
        }
        crate::utils::jwt::VersionCheck::Unavailable(e) => {
            // fail-closed：无法判定是否已吊销时，不给新令牌。
            // 见 middleware/auth.rs 里同一分支的详细理由。
            tracing::error!("refresh 时无法校验令牌版本，拒绝（fail-closed）: {}", e);
            return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
        }
    }

    // 验证用户账号仍然有效
    let still_active = if claims.role == "admin" {
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM admins WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
    } else {
        sqlx::query_as::<_, (String,)>(
            "SELECT id::text FROM merchants WHERE id = $1 AND status = 'active'",
        )
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
    };

    // ⚠️ 这里原来是 `.unwrap_or(None)` —— 数据库出错会被当成「查到了空结果」，
    // 于是返回「账号不存在或已被禁用」。问题不大（都是拒绝），
    // 但错误信息会**误导排查方向**：明明库在抖，日志上却像是用户被禁用了。
    // 现在把两种情况分开报。
    let still_active = match still_active {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("refresh 校验账号状态失败: user_id={} err={}", user_id, e);
            return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
        }
    };

    if !still_active {
        return Json(json!({"success": false, "message": "账号不存在或已被禁用"}));
    }

    // ── ② 重新读一次当前版本号 ────────────────────────────────────────────
    //
    // 为什么重新读而不是直接用 claims.ver（上面刚校验过相等）：
    // 上面那次校验读的是**可能已缓存 30 秒**的值。想象这个时序：
    //   T+0   攻击者用旧 refresh 发起刷新，校验读到缓存 ver=0 → 通过
    //   T+5   用户改密码 → 库 ver=1，缓存被 publish 成 1
    //   T+6   上面那次校验（它在 T+0 之后、可能因为 I/O 排队晚一点完成）
    //   ...
    // 与其推理「缓存窗口内不可能出错」，不如**在签发前直接查一次库**。
    // refresh 是低频操作（每 2 小时一次/设备），多一次查询完全可接受，
    // 换来的是「签发出去的新令牌一定与库一致」这个**强保证**。
    let current_ver = match crate::utils::jwt::load_token_version(&state.pool, &claims.role, &user_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("refresh 读取 token_version 失败: user_id={} err={}", user_id, e);
            return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
        }
    };

    // 签发新 Access Token
    let new_token = match generate_token(&user_id, &claims.role, &claims.email, current_ver, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };

    // 同时滚动续期 Refresh Token
    let new_refresh = match generate_refresh_token(&user_id, &claims.role, &claims.email, current_ver, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return Json(json!({"success": false, "message": "生成令牌失败"})),
    };

    Json(json!({
        "success": true,
        "token": new_token,
        "refresh_token": new_refresh
    }))
}

/// 发送密码重置验证码
async fn send_reset_code(
    State(state): State<AppState>,
    Json(body): Json<SendCodeRequest>,
) -> Json<Value> {
    if !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }

    // 检查邮箱是否已注册
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：数据库出错 → 判定「未注册」→ 返回「该邮箱未注册」。
    // 对用户来说这是最让人不安的一种错法 —— 明明账号就在那儿，
    // 系统却说「这个邮箱没注册过」，用户会开始怀疑自己记错了邮箱。
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let exists = match db_guard::optional(
        sqlx::query_as::<_, (String,)>("SELECT id::text FROM merchants WHERE email_hash = $1 LIMIT 1")
            .bind(&email_hash)
            .fetch_optional(&state.pool),
        "检查邮箱是否已注册（send_reset_code）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(v) => Some(v),
        db_guard::QueryOutcome::NotFound => None,
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };
    if exists.is_none() {
        return Json(json!({"success": false, "message": "该邮箱未注册"}));
    }

    let mut redis = state.redis.clone();
    let cooldown_key = format!("reset:cooldown:{}", body.email);

    // 60秒冷却防刷
    let in_cooldown: bool = redis.exists(&cooldown_key).await.unwrap_or(false);
    if in_cooldown {
        return Json(json!({"success": false, "message": "请求过于频繁，请60秒后再试"}));
    }

    // 生成6位数字验证码
    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Uniform::new(0u32, 10))
        .take(6)
        .map(|d| char::from_digit(d, 10).unwrap())
        .collect();

    // 存入 Redis：验证码 10 分钟过期，冷却标记 60 秒过期
    //
    // 与上面 `send_code` 完全同构 —— 验证码走 `required`（业务凭据，写不进就拒绝），
    // 冷却标记走 `protection_lost`（防刷措施，写不进只丢防刷）。两处必须一致，
    // 否则「注册能用、重置密码不能用」这种半修状态最难查。
    let code_key = format!("reset:code:{}", body.email);
    if redis_guard::required::<()>(
        redis.set_ex(&code_key, &code, 600).await,
        "写入重置验证码",
    )
    .is_err()
    {
        return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
    }
    redis_guard::protection_lost::<()>(
        redis.set_ex(&cooldown_key, "1", 60).await,
        "写入重置冷却标记",
    );

    // 发送邮件
    match send_verify_code(&state.mailer, &body.email, &code).await {
        Ok(_) => Json(json!({"success": true, "message": "验证码已发送，请查收邮件"})),
        Err(e) => {
            tracing::error!("发送密码重置验证码失败: {}", e);
            redis_guard::best_effort::<()>(redis.del(&code_key).await, "清理重置验证码");
            redis_guard::best_effort::<()>(redis.del(&cooldown_key).await, "清理重置冷却标记");
            Json(json!({"success": false, "message": "邮件发送失败，请稍后重试"}))
        }
    }
}

/// 重置密码
async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordRequest>,
) -> Json<Value> {
    if !body.email.contains('@') {
        return Json(json!({"success": false, "message": "邮箱格式不正确"}));
    }
    if body.new_password.len() < 8 {
        return Json(json!({"success": false, "message": "密码至少8位"}));
    }
    if body.code.len() != 6 || !body.code.chars().all(|c| c.is_ascii_digit()) {
        return Json(json!({"success": false, "message": "验证码格式错误"}));
    }

    let mut redis = state.redis.clone();
    let code_key = format!("reset:code:{}", body.email);

    // 从 Redis 取出验证码
    //
    // ⚠️ 同 register：`.unwrap_or(None)` 会把 Redis 故障伪装成「验证码无效或已过期」。
    // 在**重置密码**这条路上伪装尤其有害 —— 用户已经点过「忘记密码」、
    // 已经收到并输入了验证码，这时告诉他「验证码无效」，最自然的解读是
    // 「我是不是又填错了」，而不是「服务端缓存不可用」。
    let stored_code: Option<String> = match redis.get(&code_key).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("读取重置验证码失败（Redis 错误，不是「验证码不存在」）: {}", e);
            return Json(json!({"success": false, "message": "服务暂时不可用，请稍后重试"}));
        }
    };
    match stored_code {
        None => return Json(json!({"success": false, "message": "验证码无效或已过期"})),
        Some(c) if c != body.code => return Json(json!({"success": false, "message": "验证码错误"})),
        Some(_) => {
            // 同上：一次性保证属于「防护」，删不掉不阻塞重置，但必须留痕
            redis_guard::protection_lost::<()>(
                redis.del(&code_key).await,
                "消耗重置验证码（一次性保证）",
            );
        }
    }

    // 查询商户
    //
    // ⚠️ 曾用 `.unwrap_or(None)`：数据库出错 → 返回「邮箱不存在」。
    // 这是最危险的一处伪装 —— 用户看到的「这个邮箱不存在」发生在
    // 密码重置流程中，他会以为自己被删号了。
    let email_hash = EncryptedFieldsOps::generate_hash(&body.email);
    let merchant = match db_guard::optional(
        sqlx::query_as::<_, Merchant>("SELECT * FROM merchants WHERE email_hash = $1")
            .bind(&email_hash)
            .fetch_optional(&state.pool),
        "按邮箱查询商户（reset_password）",
    )
    .await
    {
        db_guard::QueryOutcome::Found(m) => m,
        db_guard::QueryOutcome::NotFound => {
            return Json(json!({"success": false, "message": "邮箱不存在"}))
        }
        db_guard::QueryOutcome::Failed => return db_guard::server_busy(),
    };

    // 加密新密码
    let new_password_hash = match hash(&body.new_password, 10) {
        Ok(h) => h,
        Err(_) => return Json(json!({"success": false, "message": "密码加密失败"})),
    };

    // 更新密码
    let result = sqlx::query(
        "UPDATE merchants SET password_hash = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&new_password_hash)
    .bind(merchant.id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => {
            // 邮箱重置密码 = 典型账号找回场景，**必须**吊销旧令牌。
            //
            // 这个场景比其他几个更关键：走这条路的人，前提就是「我可能进不去账号了」——
            // 那很可能是账号已被盗用。如果重置完密码、攻击者手里的 token 还有效，
            // 重置就完全失去意义（他不是靠密码进来的，他是靠 token）。
            //
            // 这里不补发新令牌（与 change_password 不同）：重置流程本来就不持有
            // 有效会话（否则用户不需要走邮箱找回），所以没有「操作者会掉线」的问题，
            // 直接要求重新登录即可。
            if let Err(e) = crate::utils::jwt::revoke_user_tokens(
                &state.pool,
                &mut state.redis.clone(),
                "merchant",
                &merchant.id,
            )
            .await
            {
                tracing::error!("重置密码后吊销令牌失败: merchant_id={} err={}", merchant.id, e);
                return Json(json!({
                    "success": false,
                    "message": "密码已重置，但会话清理失败，请重新登录一次以确认安全"
                }));
            }
            Json(json!({"success": true, "message": "密码重置成功，请重新登录"}))
        }
        // 同 register：**未登录可达**。原先回 `format!("重置失败: {}", e)`，
        // 把 sqlx 原文（表名/列名/约束名）直接给了任何人。
        Err(e) => db_guard::internal_error("重置密码（UPDATE merchants）", e),
    }
}
