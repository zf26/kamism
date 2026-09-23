use anyhow::Result;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::redis_guard;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub email: String,
    pub exp: i64,
    pub iat: i64,
    /// 令牌版本号 —— 吊销机制的核心。
    ///
    /// 签发时把 `merchants.token_version` / `admins.token_version` 的当前值写进来，
    /// 校验时和库里的值比对；不等即视为已吊销。
    ///
    /// ⚠️ `#[serde(default)]` 是**必须**的，不能省：
    /// 升级瞬间，所有已在线用户手里是**没有这个字段**的旧 token。
    /// 没有 default 的话反序列化会直接失败 → 所有人被登出。
    /// 有了 default，旧 token 的 ver 读作 0，而存量用户在库里也是 0
    /// （见 `009_token_version.sql` 的 DEFAULT 0）→ 平滑过渡。
    ///
    /// 换句话说：**这个 default 值 0 和迁移里的 DEFAULT 0 是一对**，
    /// 只改其中一个会让升级变成一次全站强制登出。
    #[serde(default)]
    pub ver: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RefreshClaims {
    pub sub: String,       // user id
    pub role: String,
    pub email: String,
    pub token_type: String, // 固定为 "refresh"
    pub exp: i64,
    pub iat: i64,
    /// 见 [`Claims::ver`] 的说明（两者语义相同，default 的理由也一样）
    #[serde(default)]
    pub ver: i64,
}

/// 生成 Access Token（2小时有效期）
///
/// `token_version` 取用户当前的 `token_version` 列值（见 `009_token_version.sql`）。
/// 传入时**必须现查库**，不能传写死的 0 —— 否则吊销过的用户一登录又能拿到 ver=0 的
/// token，而库里已经是 1，等于登录即失效。
pub fn generate_token(
    user_id: &Uuid,
    role: &str,
    email: &str,
    token_version: i64,
    secret: &str,
) -> Result<String> {
    let now = Utc::now();
    let exp = now + Duration::hours(2);
    let claims = Claims {
        sub: user_id.to_string(),
        role: role.to_string(),
        email: email.to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
        ver: token_version,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

/// 生成 Refresh Token（7天有效期，使用独立密钥前缀区分）
///
/// `token_version` 的约束同 [`generate_token`]。
pub fn generate_refresh_token(
    user_id: &Uuid,
    role: &str,
    email: &str,
    token_version: i64,
    secret: &str,
) -> Result<String> {
    let now = Utc::now();
    let exp = now + Duration::days(7);
    let refresh_secret = format!("{}:refresh", secret);
    let claims = RefreshClaims {
        sub: user_id.to_string(),
        role: role.to_string(),
        email: email.to_string(),
        token_type: "refresh".to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
        ver: token_version,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(refresh_secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn verify_token(token: &str, secret: &str) -> Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(token_data.claims)
}

pub fn verify_refresh_token(token: &str, secret: &str) -> Result<RefreshClaims> {
    let refresh_secret = format!("{}:refresh", secret);
    let token_data = decode::<RefreshClaims>(
        token,
        &DecodingKey::from_secret(refresh_secret.as_bytes()),
        &Validation::default(),
    )?;
    // 确保是 refresh token 而非 access token
    if token_data.claims.token_type != "refresh" {
        return Err(anyhow::anyhow!("不是有效的 Refresh Token"));
    }
    Ok(token_data.claims)
}

// ─────────────────────────────────────────────────────────────────────────────
// 令牌版本校验（吊销机制）
// ─────────────────────────────────────────────────────────────────────────────

/// 令牌版本校验的结论。
///
/// ⚠️ 为什么不用 `bool`：`bool` 会把「Redis 挂了」和「版本不匹配（已在别处登出）」
/// 压成同一个值。这两件事的**正确处理方式完全相反**：
///   - 版本不匹配 → 这是一个明确的业务判定，**拒绝**
///   - Redis 不可用 → 这是**基础设施故障**，不是用户的错
///
/// 如果压成 `bool` 并且 `Err → false`，那么 Redis 一抖动 = **全站所有人被登出**；
/// 如果反过来写 `Err → true`，那么 Redis 一挂 = **吊销机制静默失效**
/// （攻击者拿到 token 后，运维把 Redis 搞挂就绕过了吊销）。
/// 两种写法都是「安静地做错事」，所以这里把三种情况摊开。
#[derive(Debug, PartialEq, Eq)]
pub enum VersionCheck {
    /// 版本一致 —— 放行
    Valid,
    /// 版本不一致 —— 该令牌已被吊销
    Revoked { token_ver: i64, current_ver: i64 },
    /// 基础设施故障 —— 无法判定
    Unavailable(String),
}

/// 比对令牌里的 `ver` 与用户当前的 `token_version`。
///
/// 调用方必须显式处理 [`VersionCheck::Unavailable`]（见那个枚举的说明）。
///
/// `pool` 是数据库连接池（**回源用**），`cache` 是 Redis 连接。
/// 两个都得由调用方传进来 —— 这个函数不自己去找全局连接，
/// 否则在事务里被调用时会静默用上另一条连接（正是本仓库踩过的 #6 那个坑）。
///
/// 为什么用缓存而不是每次都查库：access token 的校验在**每个 API 请求**上都会跑
/// （`auth_middleware`），直接查库会给热路径加一次 `SELECT`。
/// 缓存 TTL 取 30 秒 —— 意味着**吊销最多延迟 30 秒生效**，这是刻意的取舍：
/// 吊销是「把坏挡在外面」，不是需要亚秒级生效的实时控制。
pub async fn check_token_version(
    pool: &crate::db::DbPool,
    cache: &mut redis::aio::ConnectionManager,
    role: &str,
    user_id: &Uuid,
    token_ver: i64,
) -> VersionCheck {
    let key = format!("tv:{}:{}", role, user_id);

    // 先看缓存
    let cached: Result<Option<String>, _> =
        redis::cmd("GET").arg(&key).query_async(cache).await;

    match cached {
        Ok(Some(s)) => match s.parse::<i64>() {
            Ok(v) => {
                return if v == token_ver {
                    VersionCheck::Valid
                } else {
                    VersionCheck::Revoked {
                        token_ver,
                        current_ver: v,
                    }
                }
            }
            // 缓存里是个解析不了的值 —— 有人往这个键写了脏数据。
            // 不能当 Valid（静默放行），也不能当 Revoked（误踢用户）。
            // 删掉它、按未命中处理，让下面回源查库。
            Err(_) => {
                tracing::warn!("token_version 缓存值非法，已丢弃: key={} value={:?}", key, s);
                // 删不掉就继续报 warn 并继续回源查库 —— 结论不受影响（派生态）
                redis_guard::best_effort::<()>(
                    redis::cmd("DEL").arg(&key).query_async(cache).await,
                    "丢弃非法的 token_version 缓存值",
                );
            }
        },
        Ok(None) => {} // 未命中，回源
        Err(e) => {
            // Redis 不可用时**回源查库**，而不是直接失败 —— 数据库是事实来源。
            // 这样 Redis 抖动既不会演变成全站登出，也不会演变成吊销失效。
            tracing::warn!("读取 token_version 缓存失败，回源查库: key={} err={}", key, e);
        }
    }

    // 回源：数据库才是事实来源。
    // 表名是内部常量（只有 "admins"/"merchants" 两种取值），不来自用户输入，
    // 所以这里的 format! 拼接不构成注入面。
    let table = if role == "admin" { "admins" } else { "merchants" };
    let row: Result<Option<(i32,)>, _> = sqlx::query_as(&format!(
        "SELECT token_version FROM {} WHERE id = $1",
        table
    ))
    .bind(user_id)
    .fetch_optional(pool)
    .await;

    match row {
        Ok(Some((current,))) => {
            let current = current as i64;
            // 回写缓存。写失败不影响本次判定（只是下次还得回源）。
            // 「不影响判定」是**对的**（数据库才是事实来源），但不能静默 ——
            // 否则「为什么吊销晚了几十秒 / 缓存为什么一直不命中」将来无从查起。
            // 同文件里的 `publish_token_version` 早就是这个口径（fail-open + 留痕）。
            redis_guard::best_effort::<()>(
                redis::cmd("SET")
                    .arg(&key)
                    .arg(current.to_string())
                    .arg("EX")
                    .arg(30)
                    .query_async(cache)
                    .await,
                "回写 token_version 缓存",
            );

            if current == token_ver {
                VersionCheck::Valid
            } else {
                VersionCheck::Revoked {
                    token_ver,
                    current_ver: current,
                }
            }
        }
        // 用户不存在 —— 这**不是**基础设施故障，而是「这个账号没了」。
        // 用 Revoked 语义拒绝是对的：令牌确实不该再用。
        Ok(None) => VersionCheck::Revoked {
            token_ver,
            current_ver: -1,
        },
        Err(e) => VersionCheck::Unavailable(format!("查询 token_version 失败: {}", e)),
    }
}

/// 主动把某用户的版本号推进缓存（吊销后立刻调用）。
///
/// 不调也行 —— 缓存最长 30 秒自然过期。但调用一下能让吊销近乎立即生效，
/// 代价是一次 Redis 往返。**注意这里写的是「新版本号」而不是删键**：
/// 删键只会让下一次校验回源查库（结果一样），而写新值能让后续校验全部命中缓存。
pub async fn publish_token_version(
    cache: &mut redis::aio::ConnectionManager,
    role: &str,
    user_id: &Uuid,
    new_ver: i64,
) {
    let key = format!("tv:{}:{}", role, user_id);
    let r: Result<(), _> = redis::cmd("SET")
        .arg(&key)
        .arg(new_ver.to_string())
        .arg("EX")
        .arg(30)
        .query_async(cache)
        .await;
    if let Err(e) = r {
        // 写不进去不是致命问题（30 秒后自然一致），但**必须留痕** ——
        // 否则将来排查「为什么吊销晚了几十秒」时会一无所获。
        tracing::warn!("回写 token_version 缓存失败: key={} err={}", key, e);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 吊销入口：把 token_version +1，并同步缓存
// ─────────────────────────────────────────────────────────────────────────────

/// 吊销某用户的**全部**已签发令牌。
///
/// 语义：调用后，该用户手里所有 access token（2h）与 refresh token（7天）
/// 立刻失效（对中间件的热路径而言最长延迟 30 秒，因为缓存 TTL）。
///
/// `role` 决定改哪张表（"admin" → admins，其余 → merchants）。
///
/// **返回 `Result` 而不是 `bool`**：调用方必须能区分「推进成功」和
/// 「数据库写失败」。如果一个改密码流程里吊销失败了却照样返回成功，
/// 用户会以为「我的旧密码已经不能用、旧会话也被踢了」，而实际上旧 token 还活着 ——
/// 这正是本项目最忌讳的「安静地做错事」。
pub async fn revoke_user_tokens(
    pool: &crate::db::DbPool,
    cache: &mut redis::aio::ConnectionManager,
    role: &str,
    user_id: &Uuid,
) -> Result<i64> {
    let table = if role == "admin" { "admins" } else { "merchants" };
    let row: Option<(i32,)> = sqlx::query_as(&format!(
        "UPDATE {} SET token_version = token_version + 1 WHERE id = $1 RETURNING token_version",
        table
    ))
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((new_ver,)) => {
            let new_ver = new_ver as i64;
            publish_token_version(cache, role, user_id, new_ver).await;
            Ok(new_ver)
        }
        // 用户不存在。对「改密码」这类流程来说，走到这一步说明前面已经查到过用户，
        // 那现在查不到就是并发删除 —— 罕见但不是不可能。
        // 这里明确报错而不是静默返回 Ok：调用方需要知道「吊销没做成」。
        None => Err(anyhow::anyhow!(
            "吊销失败：{} 表中不存在 id={} 的用户",
            table,
            user_id
        )),
    }
}

/// 读取用户当前的 `token_version`（登录时用来写进新签发的令牌）。
///
/// 用户不存在时返回 0 而不是报错 —— 调用方在登录流程里已经查到了这个用户，
/// 走到这里再查主要是为了拿版本号。但**要注意**：如果这里返回的 0 与库里实际值
/// 不一致（比如查询失败被吞掉），签出来的令牌立刻就是废的。
/// 所以查询失败必须冒出 `Err`，只有「查通了、但确实没有这条记录」才回落为 0。
pub async fn load_token_version(
    pool: &crate::db::DbPool,
    role: &str,
    user_id: &Uuid,
) -> Result<i64> {
    let table = if role == "admin" { "admins" } else { "merchants" };
    let row: Option<(i32,)> = sqlx::query_as(&format!(
        "SELECT token_version FROM {} WHERE id = $1",
        table
    ))
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(v,)| v as i64).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test_secret_at_least_32_chars_long_ok";

    fn uid() -> Uuid {
        Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap()
    }

    /// **最关键的一条**：升级平滑性。
    ///
    /// 模拟「升级前签发的旧 token」—— 也就是 payload 里**没有 `ver` 字段**的 token。
    /// 如果没有 `#[serde(default)]`，这个 token 会反序列化失败 → 所有在线用户被登出。
    ///
    /// 怎么造出这种 token：手工拼一个不含 ver 的 payload 去 encode。
    /// 注意不能复用 `Claims`（它现在有 ver 字段），所以用 serde_json::Value 构造。
    #[test]
    fn legacy_token_without_ver_field_still_decodes_as_zero() {
        let now = Utc::now();
        let payload = serde_json::json!({
            "sub": uid().to_string(),
            "role": "merchant",
            "email": "old@example.com",
            "iat": now.timestamp(),
            "exp": (now + Duration::hours(1)).timestamp(),
            // ⚠️ 故意不带 "ver"
        });
        let legacy = jsonwebtoken::encode(
            &Header::default(),
            &payload,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();

        let claims = verify_token(&legacy, SECRET).expect("旧 token 必须仍然可解析");
        assert_eq!(
            claims.ver, 0,
            "缺失 ver 必须回落为 0 —— 与 009 迁移的 DEFAULT 0 配对，否则升级即全站登出"
        );
    }

    /// 反向确认：`ver` 确实被写进了 token，而且能读回来。
    /// 如果签名函数漏掉了 ver（比如仍传默认值），刷新后的令牌会立刻失效 —— 这条能抓住。
    #[test]
    fn ver_is_embedded_and_round_trips() {
        let t = generate_token(&uid(), "merchant", "a@b.c", 7, SECRET).unwrap();
        let claims = verify_token(&t, SECRET).unwrap();
        assert_eq!(claims.ver, 7, "token 里的 ver 必须等于签发时传入的值");

        let r = generate_refresh_token(&uid(), "merchant", "a@b.c", 3, SECRET).unwrap();
        let rc = verify_refresh_token(&r, SECRET).unwrap();
        assert_eq!(rc.ver, 3, "refresh token 同理");
    }

    /// access token 与 refresh token 用不同密钥派生前缀，**不能互相冒用**。
    /// 这条防的是「把 access token 当 refresh 提交」这种越权尝试。
    #[test]
    fn refresh_and_access_tokens_are_not_interchangeable() {
        let access = generate_token(&uid(), "merchant", "a@b.c", 0, SECRET).unwrap();
        assert!(
            verify_refresh_token(&access, SECRET).is_err(),
            "access token 不能被当成 refresh token 接受"
        );

        let refresh = generate_refresh_token(&uid(), "merchant", "a@b.c", 0, SECRET).unwrap();
        // 反过来：refresh 也不该被 access 校验接受（密钥前缀不同 → 验签失败）
        assert!(
            verify_token(&refresh, SECRET).is_err(),
            "refresh token 不能被当成 access token 接受"
        );
    }

    /// 篡改 ver 会让验签失败 —— 确认客户**无法自己改版本号**来绕过吊销。
    /// 这条是吊销机制的安全前提：如果 ver 可篡改，整个方案没有意义。
    #[test]
    fn tampering_ver_breaks_signature() {
        let t = generate_token(&uid(), "merchant", "a@b.c", 0, SECRET).unwrap();
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT 应为三段");

        // 把 payload 解出来、改成 ver=999、重新 base64 —— 但**不重签名**
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        v["ver"] = serde_json::json!(999);
        let forged = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&v).unwrap());
        let forged_token = format!("{}.{}.{}", parts[0], forged, parts[2]);

        assert!(
            verify_token(&forged_token, SECRET).is_err(),
            "改过 ver 却没重签的 token 必须验签失败 —— 否则吊销可被绕过"
        );
    }

    /// 不同 master secret 签出的 token 不能互认（防止换密钥后旧 token 被接受）。
    #[test]
    fn token_from_other_secret_is_rejected() {
        let t = generate_token(&uid(), "merchant", "a@b.c", 0, SECRET).unwrap();
        assert!(verify_token(&t, "another_secret_at_least_32_chars_x").is_err());
    }
}
