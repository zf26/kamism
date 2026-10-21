# KamiSM 项目体检报告

**体检日期**：2026-10-20
**代码版本**：`b0f90ed`（main，含本地未提交的续期功能与人机验证改动）
**体检方式**：全量静态审阅 + 关键路径核查 + 本地实跑验证（单元测试、启动、接口）

---

## 一、一句话结论

**工程底子明显好于同类项目，问题不在"写得糙"，而在"长大了但骨架没跟着长"。**

代码里对"静默失败"的警惕程度是我见过的项目里少见的高——几乎每个 `unwrap_or_default` 都有一句注释解释它曾造成过什么事故。加密、哈希、令牌吊销、支付幂等、并发激活锁这些**真正容易出事的地方都做对了**。

真正的风险集中在三处：

1. **资金侧有一处会直接让用户"付了钱买不到时长"的时间基准错误**（P0）
2. **性能设计全部假设"数据量很小"**——卡密列表、激活搜索都是"全量解密再分页"，数据一上万就开始雪崩
3. **部署侧没有任何可观测性，也没有一层兜底**——MQ 断连要重启进程、没有 CI、没有指标、没有优雅关闭

---

## 二、评分卡

> 下表是**发现时**的评分。P0 五项修复后，安全升至约 7.8、资金一致性升至约 8.2、
> 性能升至约 6.0、测试升至约 5.2（126 个单测 + 一份 42 项端到端脚本），
> 综合约 **6.9**。真正的短板已经转移到 P1 的运维层：MQ 无重连、无指标、无 CI、无优雅关闭。

| 维度 | 评分 | 判断依据 |
|---|---|---|
| 安全 | **7.0** / 10 | 加密/哈希/令牌吊销/启动期强校验做得很扎实；但 SSRF、日志泄露凭据、无审计日志是硬伤 |
| 资金与数据一致性 | **7.0** / 10 | 支付幂等与激活并发锁是教科书级；但续期时间基准有正确性 bug，webhook 无重试 |
| 性能与可扩展性 | **4.5** / 10 | "全量解密再分页"两处 + 单实例架构（WS/缓存/调度器全在进程内） |
| 可用性与运维 | **5.0** / 10 | MQ 是启动硬依赖且无重连；无优雅关闭、无指标、无 CI |
| 工程质量 | **7.5** / 10 | 注释质量极高、无 TODO 残留、`cargo build` 零 warning；但巨型文件、错误语义不统一 |
| 测试 | **4.5** / 10 | 109 个单测全绿，但 **0 个集成测试、0 个前端测试**，路由层完全靠手工验证 |
| **综合** | **6.0** / 10 | 骨架能撑住当前规模，撑不住下一个量级 |

---

## 三、P0：建议本周处理

> **状态：5 项已全部修复（2026-10-20）。** 每节末尾附「修复方式」与验证结论；
> 端到端验证脚本 `.workbuddy/p0-test.cjs`，42/42 通过。
> 下文正文保留**发现时**的描述，便于回溯问题成因。

### P0-1 续期的时间基准错误——用户付钱可能买不到时长 ✅

`src-tauri/src/routes/payments.rs:1368`

```sql
plan_expires_at = COALESCE(plan_expires_at, NOW()) + ($1 || ' days')::INTERVAL
```

三个独立的问题，都由这一行产生：

| 场景 | 实际结果 |
|---|---|
| 商户 40 天前就过期了，现在付 30 天 | `基准 = 40天前` → 新到期时间仍在 10 天前 → **下一轮扫描立刻降级，30 天白买** |
| 永久专业版（`plan_expires_at = NULL`）付 30 天 | `COALESCE(NULL, NOW())` → 把**永久**变成 30 天，**降级** |
| 管理员在后台"升专业版"（`admin.rs`，`NOW() + N days`） | 覆盖式：原本还剩 100 天，选 30 天会**缩短**成 30 天 |

而本次新加的续期接口用的是 `GREATEST(plan_expires_at, NOW()) + N days`，语义是对的。**结果是同一个系统里现在存在四种有效期语义**，其中三种会在特定场景下损害用户或平台。

> 值得强调：这不是"少算了几天"，是"用户付了钱、界面显示成功、到期日却没动"。属于最难解释、也最容易演变成投诉/退款的那类问题。

**建议**：统一抽一个函数 `extend_plan(pool, merchant_id, days)`，全站只走它，语义固定为 `GREATEST(plan_expires_at, NOW()) + days`，永久版单独特判（拒绝或明确提示）。

**修复方式**：新增 `src-tauri/src/utils/plan.rs`，一条 CTE 语句同时完成「读旧值 / 判永久 / 写新值」：

| 原状态 | 新口径 |
|---|---|
| 专业版且在有效期内 | 在**剩余时间**上叠加 |
| 专业版但已过期 | 从**现在**起算（过期那段不补、不倒扣） |
| 免费版 | 从**现在**起算，不继承历史到期时间 |
| 永久专业版 | **不改动**，返回 `ExtendOutcome::Permanent` 由调用方提示 |

`payments.rs`（支付落地，同事务内）、`admin.rs` 的升级与续期三条路径全部改走它。永久版被 `WHERE NOT (b.plan = 'pro' AND b.plan_expires_at IS NULL)` 挡在 UPDATE 之外——这一条正是为了消灭「`rows_affected=1` 但什么都没变」的假成功。

**验证**：免费版开 30 天 → 专业版再加 10 天 → 剩余 **40** 天（旧行为是覆盖成 10）；续 5 天 → 45 天；设永久后再设置/续期均被拒；降免费后开 7 天 → 剩余 7 天。

---

### P0-2 Webhook 地址只校验了协议前缀 → 任意注册商户可探测内网 ✅

`src-tauri/src/routes/webhooks.rs:137-142`

```rust
if !body.url.starts_with("http://") && !body.url.starts_with("https://") { ... }
```

注册是开放的（邮箱验证码即可），所以**任何一个注册用户**都能把 webhook 指到：

- `http://127.0.0.1:5432`（本机 PG）、`http://redis:6379`、`http://app:9527/health`
- `http://169.254.169.254/latest/meta-data/`（云厂商元数据，可导致凭证泄露）
- 内网任意网段做端口扫描

判据来自响应结果：投递日志里会记录 `status=...`，而商户可以通过"有没有收到 webhook"间接判断连通性；更直接的是——**reqwest 默认跟随重定向**（`webhooks.rs:432` 只设了超时，没设 `redirect::Policy::none()`），所以即使加了域名黑名单，一个 302 也能跳回内网。

**建议**（三层，缺一不可）：
1. 解析 URL 得到 IP，拒绝私有段 / 回环 / 链路本地 / 元数据地址（含 IPv6 与 `::ffff:` 映射）
2. `redirect::Policy::none()`，或跟随但每跳重新校验
3. 更稳的做法是把 webhook 投递放到独立的 egress 代理/worker，与主进程网络隔离

**修复方式**：新增 `src-tauri/src/utils/url_guard.rs`，在**保存时**与**每次投递前**各校验一次；reqwest 改为 `redirect::Policy::none()`。禁区覆盖：

- 回环（`127/8`、`::1`）、私有（`10/8`、`172.16/12`、`192.168/16`、`fc00::/7`）
- 链路本地（`169.254/16`，含元数据 `169.254.169.254`、`fe80::/10`）
- `0.0.0.0/8`、`100.64/10`、组播、保留段、IPv4-mapped IPv6（`::ffff:127.0.0.1` 会被还原成 v4 再判）

域名解析后**只要有任一地址落在禁区就整体拒绝**——不做「挑一个合法的用」，否则可控 DNS 同时返回 `[公网IP, 127.0.0.1]` 就能绕过。解析结果被 pin 住（锁到校验收的那个 IP），避免 DNS rebinding 在两次解析之间换答案。

**验证**：本机 PG、元数据地址、`[::1]`、`10.0.0.5`、`[::ffff:127.0.0.1]`、`192.168.1.1` 六种全部拒绝且提示明确；公网 IP 正常保存。

---

### P0-3 启动失败日志会把 RabbitMQ 密码打进日志 ✅

`src-tauri/src/lib.rs:103`

```rust
.map_err(|e| anyhow::anyhow!("RabbitMQ 连接失败: {} (AMQP_URL={})", e, amqp_url))?;
```

`AMQP_URL` 形如 `amqp://user:password@host:port/vhost`，**整条打进 stderr**。`lib.rs:96` 的 Redis 同理。

日志会被 `docker logs`、supervisor、日志采集、排障截图带走——凭据泄露最常见的路径就是这里。这类事故的特点是"发生时不觉得，事后查不出来"。

**建议**：统一一个 `redact_url()`，输出 `amqp://user:***@host:port/vhost`。顺带检查 `.env` 的错误回显是否也有同类问题。

**修复方式**：`utils/mask.rs` 新增 `redact_url()`，优先按 URL 语义解析（能正确处理密码里的 `%40`、`:` 等特殊字符），解析失败时回退到字符串裁剪。`lib.rs` 两处错误信息改为 `REDIS_URL={}` / `AMQP_URL={}` 传入脱敏后的值。

---

### P0-4 卡密列表与激活搜索：全量解密后再分页 ✅

`src-tauri/src/routes/cards.rs:244-300`、`src-tauri/src/routes/activations.rs:76-112`

两处是同一个模式：

```
SELECT ... WHERE merchant_id = $1        -- 不分页
  ↓ 逐行 AES-256-GCM 解密（每行一次）
  ↓ 内存里 contains 过滤
  ↓ 内存里切片分页
```

代价是 **O(全表) 的密码学运算**，而每次搜索都重复一遍。

| 数据量 | 单次搜索的代价（估算） |
|---|---|
| 1 千张卡 | 可忽略 |
| 1 万张 | 约 1 万次 AES-GCM，几十 ms，尚可 |
| 10 万张 | 10 万次解密 + 百 MB 级内存，秒级卡顿，并发几个就雪崩 |
| 100 万张 | 基本不可用 |

而同一个文件的 `export_cards_csv`（`cards.rs:400`）**已经做成了流式导出**——说明团队知道正确做法，只是列表接口没跟上。

**建议**：搜索改为"精确查 `code_hash`"（已有唯一索引）+ "前缀搜索走独立索引列"，把 `contains` 模糊搜索降级为只在前缀匹配之上做。真要全文模糊，就该上专门的检索（或至少把解密结果缓存成明文检索列，加密边界另议）。

**修复方式**：分两层，都不再做「全量 → 内存分页」：

- **① 精确命中**：走已有 `code_hash` 唯一索引，一次查找 + 一行解密。大小写各算一次哈希（卡密按大写生成，用户常粘小写），输入 < 6 字符时跳过（不可能是完整卡密）。这条覆盖了用户最常见的行为——把卡密粘过来查。
- **② 模糊匹配**：仍要解密（`code_encrypted` 是 AES-GCM，SQL 没法 `LIKE`，这是加密存储的固有代价），但加了明确的 `SEARCH_SCAN_LIMIT` 上限，超限时返回 `truncated: true`，前端提示「结果可能不完整，请输入更完整的关键词」。

不给模糊匹配硬造索引的原因：想让 `contains` 走索引就得存明文或明文 n-gram，等于把「拿到库 = 拿到卡密」这个前提请回来，与 `code_encrypted` 的设计目的直接冲突。真正的解法是独立检索服务，不在这轮范围内。

**验证**：完整卡密精确命中 `total=1 exact=true`；小写完整卡密同样命中；4 字符片段走模糊匹配能搜到且 `truncated=false`；不存在的卡密返回空列表。

---

### P0-5 管理员既不能改密码，也没有审计日志 ✅

- **改密**：`admin.rs` 全文没有任何 `UPDATE admins SET password_hash` 的路由。商户端有（`merchant.rs:295`），管理员没有。忘记/怀疑泄露时，唯一的办法是直接连库改。
- **审计**：全仓库、全部迁移文件里 **0 处** `audit`。而管理员能做的事包括：给任意商户开专业版、改到期时间、禁用商户、删除商户、改支付渠道配置、改 OAuth 密钥。

一个能发放付费权益的管理后台没有操作留痕，意味着：内部滥用查不出、误操作无法还原、争议时拿不出证据。

**建议**：加 `admin_audit_logs(id, admin_id, action, target_type, target_id, before, after, ip, created_at)`，在管理端所有写操作上落一条。成本很低（一个中间件或几个 helper），价值极高。

**修复方式**：

- **改密**：新增 `POST /admin/me/password`，校验原密码 → 更新哈希 → **递增 `token_version`**（复用已有的三态吊销机制，改密即踢掉所有旧会话）→ 返回新令牌避免当前会话自断。前端新增「账号安全」页（`pages/admin/Settings.tsx`）。
- **审计**：迁移 `012_admin_audit_logs.sql` + `middleware/audit.rs`，挂在 5 个管理端路由组（`admin`、`oauth_admin`、`payment_admin`、`plan_config`、`subscription_plan`）。只记**写操作**（POST/PUT/PATCH/DELETE），GET 不入库；`detail` 走**白名单字段**（如 `plan`/`expires_days`/`days`），不整包存 body——否则密码、密钥会一起落库；记录操作人 ID/邮箱与来源 IP（取 `trusted_proxies.resolve` 的结果，不是 TCP 对端）。写入失败只记 warn，不影响业务。新增「操作审计」页（`pages/admin/AuditLogs.tsx`）。

**验证**：原密码错误被拒；改密成功 → 旧密码失效、旧令牌 401、新密码可登录；审计查到 25 条记录，方法集合仅 `PATCH/POST/DELETE`，`detail` 中无任何密码字段，操作人邮箱与来源 IP 均正确。

---

## 四、P1：本季度应处理

### 4.1 Webhook 没有重试，通知丢了就是丢了

`webhooks.rs:465` 的日志自己写着"当前实现无重试"。

对卡密系统来说，webhook 是**产品价值的核心承诺**——商户的业务系统靠它发卡、发货、开通会员。一次网络抖动 = 一笔业务丢失，且商户只能靠自己轮询兜底（而这恰恰是 webhook 要解决的问题）。

**建议**：投递失败写 `webhook_deliveries` 表（含 payload、下次重试时间、尝试次数），由 worker 指数退避重试 3~5 次，超过阈值标记失败并可在后台查看。顺带解决"商户问'为什么没收到通知'时无法取证"的问题。

### 4.2 MQ 连接断了只能重启进程

`utils/mq.rs:32` 在启动时建一次连接，之后**没有任何重连逻辑**：

- 发布侧：TCP 断掉后 `basic_publish` 持续失败，降级消息发不出去
- 消费侧：worker 的重启循环拿的是**同一个已死的 channel**，`create_consumer` 永远失败 → 1→2→4→…→60 秒退避空转
- 另外没有 `confirm_select` / `wait_for_confirms`，发布"成功"不代表 broker 收到

`lib.rs` 的 worker 重启注释已经预判了"消息堆在队列里无人消费，日志看起来完全正常"，但漏了"channel 本身不可恢复"这一层。

**建议**：把连接放进可重建的持有者（`Arc<Mutex<Option<Channel>>>` 或 lapin 的 `Connection` + 按需 `create_channel`），发布失败时重建一次再重试；worker 循环每轮重新建 channel 而不是复用。

### 4.3 CORS 默认放开到 `*`

`lib.rs:231`：`ALLOWED_ORIGIN` 未设置时 `allow_origin(Any)`。而 `docker-compose.yml` **没有传这个变量**，`env.example` 里**连这个变量的名字都没出现**——所以默认部署（也就是照着 README 做的部署）就是通配。

当前鉴权走 Bearer token 而非 Cookie，危害有限（没有 CSRF 面），但这意味着任何第三方站点都能直接调用你的 API、读取响应。属于"没出事只是因为还没人利用"的配置。

**建议**：默认改为从 `FRONTEND_URL` 推导；未配置时**拒绝跨域**而不是放开，并在启动日志里明确打印当前策略（像 `TRUSTED_PROXIES` 那样）。

### 4.4 全站 100MB 请求体，且未鉴权接口同样适用

`lib.rs:274` 只有一层 `RequestBodyLimitLayer::new(100MB)`，注释里写的"通用 API 2MB"并未实现。

`/auth/register`、`/auth/login`、`/v1/activate` 这些**完全公开**的端点都接受 100MB body。攻击者不需要任何凭据，就能用少量并发把带宽和内存打满（限流是 10~60 次/分钟，但每次 100MB）。

**建议**：全局降到 2MB，只在确实需要大 body 的路由上（当前其实一个都没有——没有文件上传端点）单独放开。

### 4.5 真正的 HTML 页面没有任何安全响应头

`middleware/security.rs` 把 CSP / X-Frame-Options / HSTS 都注入了，**但只作用于 API 的 JSON 响应**。用户实际打开的 HTML 由 Nginx 直接返回，而 `nginx.conf` 与 `nginx.standalone.conf` 里**一条 `add_header` 都没有**（已核对）。

叠加另一个事实：token 存在 `localStorage`（33 处直接读写）。一旦出现 XSS，攻击者可以直接取走 7 天有效的 refresh token，而没有任何 CSP 拦得住。

**建议**：在 Nginx 上补齐 `Content-Security-Policy`、`X-Frame-Options`、`X-Content-Type-Options`、`Referrer-Policy`、`Strict-Transport-Security`、`server_tokens off`。这一步比在 Rust 中间件里加要有效得多。

### 4.6 bcrypt 在 async 运行时上同步执行

`auth.rs` 登录路径：`verify(&body.password, ...)`（`bcrypt::verify`，cost 12 ≈ 250ms CPU）。这是**同步阻塞调用**，直接跑在 tokio worker 线程上。

单次登录限流是 10 次/分钟/IP，但攻击者换 IP 就能并发。**几十个并发登录 = 几十个 worker 线程全被 bcrypt 占满**，此时全站所有接口一起变慢——这是典型的"用一个便宜的请求打垮整个服务"。

**建议**：`tokio::task::spawn_blocking` 包住 hash/verify；或者给登录单独一个小 runtime / 信号量，把 bcrypt 的并发上限钉死。

### 4.7 README 里的 `TRUSTED_PROXIES` 网段是错的 ✅

| 位置 | 值 |
|---|---|
| `README.md:128-129` | `172.16.0.0/12` |
| `env.example:37` | `172.28.0.0/16` |
| `docker-compose.yml:124` | `subnet: 172.28.0.0/16` |

照 README 抄的人，配置不会报错，但**限流会退化成"全站共用一个 IP 的额度"、IP 黑名单彻底失效**——正是 `env.example` 里用大段文字警告的那个故障。这种"文档把正确配置写错了"的问题，比代码 bug 更难发现。

**建议**：README 改为直接引用 `env.example`，不要再复制一份可变的值。

**修复方式**：`README.md` 改按部署拓扑分别标注——多容器 `172.28.0.0/16`（与 `docker-compose.yml` 末尾钉死的 `subnet` 一致）、单容器与同机 Nginx `127.0.0.1`、异机 Nginx 填其出口 IP，示例值同步为 `172.28.0.0/16,127.0.0.1`；并说明"只写真正会转发请求的那一段"，写大了等于放开 `X-Forwarded-For` 伪造面（原先的 `/12` 覆盖 172.16~172.31，比实际需要的宽 16 倍）。"方式二"部署步骤下补了一条网段联动提醒：改动 `docker-compose.yml` 的 `subnet` 必须同步改 `.env`。

### 4.8 错误一律返回 HTTP 200

`auth.rs` 里 **70 处** `Json(json!({"success": false}))`、**0 处** 指定状态码。业务失败（密码错、验证码错、卡密不存在、余额不足）全部是 `200 OK`。

后果：
- 监控系统看不到错误率，只能靠解析 JSON
- Nginx/网关的重试、熔断、告警全部失效
- 前端拦截器只能靠 `success` 字段判断，与 HTTP 生态脱节

**建议**：新接口按语义返回 400/401/403/404/409/422；存量接口可在 BFF 层统一映射，不必一次性改完。

### 4.9 没有 CI，也没有任何集成测试

- 全仓库 **0 个** `.github/workflows`
- `src-tauri/tests/` **不存在**，`#[tokio::test]` **0 个**
- 前端 **0 个** 测试文件（无 vitest / 无 playwright）

109 个单元测试全部集中在 `utils/` 与少量纯函数（`cards.rs` 的过滤拼接、`webhooks.rs` 的签名、`public_api.rs` 的哈希），**没有任何一个测试会启动 HTTP 服务、连数据库、跑一遍真实请求**。

这次"续期功能"就是靠手写脚本端到端验证的——验证质量取决于人，不取决于机器，且不能防回归。

**建议**（按性价比排序）：
1. 先加 GitHub Actions：`cargo fmt --check` + `cargo clippy -D warnings` + `cargo test` + `tsc --noEmit` + `vite build`。半天的事，能拦住 80% 的低级回归
2. 再用 `sqlx::test` + `axum::Router` 写 10 个左右的关键路径集成测试（登录、激活、支付回调幂等、续期）
3. 前端先不追求覆盖率，把 `api.ts` 的令牌续期状态机补上单测即可

### 4.10 没有指标，也没有请求级追踪

全仓库无 `prometheus` / `opentelemetry` / `SetRequestIdLayer`。`/health` 是唯一的对外信号，且它每次都要真的查一次 DB 和 Redis（未鉴权，可被用来放大负载）。

出问题时只能 `docker logs | grep`，且日志是纯文本、无 request id、无法把"一次用户投诉"和"多条日志"关联起来。

**建议**：至少加 request id 中间件 + 结构化日志（`tracing` 已有，加个 `json` layer 即可）；指标先上 5 个（QPS、p95、5xx 率、连接池占用、队列堆积）就够用。

---

## 五、P2：结构性问题，需要规划

### 5.1 单实例架构是硬约束，但没有写下来

以下全部是**进程内状态**，部署两个副本就会出错：

| 组件 | 位置 | 多副本后果 |
|---|---|---|
| WebSocket 注册表 | `utils/ws.rs`（`HashMap`） | A 发出的站内信推不到连在 B 上的用户 |
| 到期扫描器 | `lib.rs:221`（tokio interval） | 两个实例各扫一遍 → 依赖 Redis 锁兜底 |
| OAuth / 支付配置缓存 | `middleware/auth.rs:35-36` | 后台改了配置，另一个实例仍是旧值 |
| 限流计数 | Redis（唯一共享的） | — |

这些都有各自的兜底（Redis 分布式锁、`updated_at` 乱序校验），说明作者是想过的。但**没有任何文档说明"本系统只能单实例部署"**，下一个人加副本时会踩得很惨。

**建议**：README 明确写一节"水平扩展约束"；中期把 WS 广播改走 Redis Pub/Sub 或 MQ，缓存加失效广播。

### 5.2 巨型文件

| 文件 | 行数 |
|---|---|
| `routes/payments.rs` | 1502 |
| `routes/public_api.rs` | 1287 |
| `routes/cards.rs` | 1258 |
| `routes/auth.rs` | 932 |

注释密度极高（部分文件注释占比接近一半），可读性其实不差，但**单文件 1500 行意味着任何改动的影响面都难以静态评估**。`payments.rs` 里混了三个支付渠道的配置、签名、回调、订单 CRUD。

**建议**：按渠道拆 `payments/{mbdpay,xorpay,alipay}.rs`，共享部分留 `payments/mod.rs`。纯搬移，零逻辑变更，风险最低。

### 5.3 MASTER_KEY 被"换掉"时没有任何检测

`utils/kms.rs:218` 的注释自己承认了这个缺口：

> 只在 `kms.is_auto_generated()` 时……**配了另一把 MASTER_KEY**（最常见的轮换场景）它拦不住

即：如果运维误把 `MASTER_KEY` 换成另一个合法格式的 64 位 hex，启动**正常通过**，然后所有加密字段在读取时静默失败。

**建议**：在 `encryption_keys` 表里存一个不可逆的密钥指纹（如 `SHA256(master_key)` 前 8 字节），启动时比对，不一致直接拒绝启动——和现有的 `lookup_pepper_fingerprint` 是同一个思路，照抄即可。

### 5.4 没有优雅关闭

`lib.rs:324` 直接 `axum::serve(...).await`，没有 `with_graceful_shutdown`。发版时会直接切断在途请求（用户看到的是"网络错误"而不是"服务重启中"）；worker 也可能在消息处理中途被杀（未 ack 会重投，但重复处理依赖幂等）。

**建议**：`axum::serve(...).with_graceful_shutdown(shutdown_signal())`，同时给 worker 一个 `CancellationToken`。

### 5.5 邮件只用于发验证码

`utils/mailer.rs` 只有一个 `send_verify_code`。缺的是：

- 支付成功收据（B 端用户财务对账必需）
- 套餐到期前提醒（当前只有站内信，而 `messages.rs:628` 的注释自己说"用户再也不点开站内信"）
- 异常登录提醒（异地/新设备）

对一个收钱的 B 端产品，这三封邮件的缺失会持续产生客服成本。

### 5.6 文档与仓库卫生

- `ENCRYPTION_GUIDE.md` / `ENCRYPTION_IMPLEMENTATION.md` / `ENCRYPTION_SUMMARY.md` 三份并存，内容是同一件事的三个版本，容易互相矛盾
- `.gitignore` 里写了 `Cargo.lock` / `package-lock.json`，但两个文件其实**已被跟踪**——下次有人 `git rm --cached` 就会把锁文件删掉，破坏可复现构建
- 本地 `.env` 的 `ADMIN_PASSWORD` 只有 13 位且以 `1234` 开头（强度偏弱，虽然通过了启动校验）

---

## 六、已做对的地方（不要在重构中破坏）

这部分是我特意列出来的——它们比上面的问题更难得：

1. **加密字段的查找哈希**：`HMAC-SHA256(pepper, value)` + 启动期版本校验。绝大多数项目在这里会退化成"要么明文可搜、要么完全搜不了"，这套方案两头都保住了。
2. **支付回调幂等**：`FOR UPDATE` → 判状态 → 验金额 → 更新，且事务内完成，还带"`WHERE status` 双保险"。这段代码经得起审。
3. **激活并发控制**：从"锁 activations 行"改成"锁 cards 行"，并在注释里写清了为什么前者在 0 行时什么锁都拿不到。这是踩过坑才写得出的东西。
4. **令牌吊销三态**（`Valid` / `Revoked` / `Unavailable`），且明确选择 fail-closed。没有把"基础设施故障"和"令牌失效"混成一个 bool。
5. **启动期拒绝可预测配置**：`JWT_SECRET` 占位符、`ADMIN_PASSWORD` 公开默认口令、`MASTER_KEY` 与存量数据不匹配，全部在启动时大声失败，并给出可执行的修复命令。`utils/config.rs` 的 16 个单测覆盖得很到位。
6. **对"静默失败"的系统性警惕**：几乎每处 `.unwrap_or_default()` 的移除都留了事故说明。这种注释密度是资产，不是负担。

---

## 七、优先级总表

| # | 问题 | 影响 | 修复成本 | 优先级 |
|---|---|---|---|---|
| 1 | 续期时间基准四种语义（付钱买不到时长/永久被降级） | 收入 + 信任 | 低（抽一个函数） | **P0** |
| 2 | Webhook SSRF（内网探测/云元数据） | 安全 | 低（校验 + 禁重定向） | **P0** |
| 3 | 日志泄露 AMQP/Redis 密码 | 安全 | 极低 | **P0** |
| 4 | 卡密/激活全量解密分页 | 性能天花板 | 中 | **P0** |
| 5 | 管理员无改密 + 无审计日志 | 合规/风控 | 低-中 | **P0** |
| 6 | Webhook 无重试 | 产品核心承诺 | 中 | P1 |
| 7 | MQ 无重连 / 无 publisher confirm | 可用性 | 中 | P1 |
| 8 | CORS 默认 `*` | 安全配置 | 极低 | P1 |
| 9 | 全站 100MB body | 可用性（DoS） | 极低 | P1 |
| 10 | HTML 无安全头 + token 在 localStorage | 安全纵深 | 低 | P1 |
| 11 | bcrypt 阻塞 async runtime | 可用性 | 低 | P1 |
| 12 | README `TRUSTED_PROXIES` 写错 | 限流失效 | 极低 | P1 ✅ |
| 13 | 错误全 200 | 可观测性 | 中（可渐进） | P1 |
| 14 | 无 CI / 无集成测试 | 长期质量 | 低（CI）/ 中（测试） | P1 |
| 15 | 无指标 / 无 request id | 运维 | 中 | P1 |
| 16 | 单实例架构无文档约束 | 扩展性 | 低（文档）/ 高（改造） | P2 |
| 17 | 巨型文件 | 可维护性 | 中（纯搬移） | P2 |
| 18 | MASTER_KEY 换钥无检测 | 数据安全 | 低 | P2 |
| 19 | 无优雅关闭 | 发布体验 | 低 | P2 |
| 20 | 邮件只有验证码 | 产品 | 中 | P2 |

---

## 八、建议路线

**第一批（P0，一次交付）**
1. 抽 `extend_plan()` 统一有效期语义，四个入口全部改走它
2. Webhook URL 三重校验（IP 段 + 禁重定向 + 超时已有）
3. 统一 `redact_url()`，清掉日志里的凭据
4. 卡密/激活搜索改 `code_hash` 精确匹配 + 前缀，去掉内存全量解密

**第二批（P1 中成本最低的一批，一天内可完成）**
- README 网段修正、CORS 默认收紧、body limit 降到 2MB、Nginx 补安全头、bcrypt 上 `spawn_blocking`、加 GitHub Actions

**第三批（P1 需要设计的）**
- Webhook 重试表 + worker、MQ 可重建连接、审计日志表、指标与 request id

**第四批（P2，随版本迭代）**
- 拆 `payments.rs`、MASTER_KEY 指纹校验、优雅关闭、水平扩展方案

---

## 附录：核查证据

| 结论 | 证据位置 |
|---|---|
| 续期基准错误 | `src-tauri/src/routes/payments.rs:1368` |
| Webhook 仅校验前缀 | `src-tauri/src/routes/webhooks.rs:137-142` |
| Webhook 无重试 | `src-tauri/src/routes/webhooks.rs:465`（日志原文） |
| 日志含 AMQP 密码 | `src-tauri/src/lib.rs:103`、`:96` |
| 全量解密分页 | `cards.rs:244-300`、`activations.rs:76-112` |
| 对比：流式导出 | `cards.rs:400` |
| CORS 默认 Any | `lib.rs:231-235`（compose 与 env.example 均未出现 `ALLOWED_ORIGIN`） |
| 100MB body | `lib.rs:274`（仅此一层） |
| 安全头仅作用于 API | `middleware/security.rs` + `nginx.conf`（无 `add_header`） |
| MQ 单连接 | `utils/mq.rs:32-50` |
| bcrypt 同步调用 | `routes/auth.rs:500+`、`merchant.rs:295` |
| 无管理员改密 | `routes/admin.rs` 全文无 `UPDATE admins SET password_hash` |
| 无审计 | 全仓库 `grep audit` 为空 |
| 无 CI | `.github/` 不存在 |
| 无集成测试 | `src-tauri/tests/` 不存在，`#[tokio::test]` = 0 |
| 109 单测全绿 | `cargo test -p kamism --lib` → 109 passed |
| README 网段错 | `README.md:128` vs `docker-compose.yml:124` |
| 错误全 200 | `auth.rs`：70 处 `Json(json!(` / 0 处 `StatusCode::` |
| 单实例约束 | `utils/ws.rs`（进程内 map）、`lib.rs:221`（进程内调度） |
