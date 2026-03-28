<div align=center><img src="https://oss.fly-fly.fun/ext/kamism.png" width = "200" height = "200"></div>

# KamiSM — 卡密管理系统

> 基于 Tauri 2.0 + Rust (Axum) + React + PostgreSQL + Redis + RabbitMQ 构建的卡密即服务（KaaS）平台。
> 支持多商户隔离、多级代理体系、风控管理、字段级数据加密等企业级特性。

---

<div align=center><img src="https://oss.fly-fly.fun/ext/kamiuser.jpg" ></div>

## 简介

KamiSM 是一个面向个人开发者和企业的卡密管理平台，支持多商户、多应用、多设备授权。商户可在平台上创建应用、批量生成卡密，终端用户通过开放 API 激活和验证卡密，实现对自研软件的授权控制。平台支持免费版与专业版订阅，管理员可控制商户套餐及有效期，到期后通过 RabbitMQ 异步降级，不影响主进程性能。

---

## 系统架构

```
┌─────────────────────────────────┐     ┌────────────────────────────────────┐
│       用户电脑                    │     │           云服务器                   │
│                                 │     │                                    │
│  ┌─────────────────────────┐   │     │  ┌──────────────────────────────┐  │
│  │   Tauri 桌面客户端        │   │     │  │   kamism-server (Axum)       │  │
│  │   (纯前端 React UI)       │──HTTP──│  │   REST API + SSE              │  │
│  │   无后端服务               │   │     │  └──────────┬─────────────────┘  │
│  └─────────────────────────┘   │     │             │                    │
└─────────────────────────────────┘     │  ┌──────────▼─────────────────┐  │
                                        │  │   PostgreSQL                │  │
┌─────────────────────────────────┐     │  │   Redis（缓存/限速/分布式锁）  │  │
│    第三方软件（商户的软件）         │     │  │   RabbitMQ（异步降级队列）    │  │
│   调用 /api/v1/verify 验证卡密   │─────│  └────────────────────────────┘  │
└─────────────────────────────────┘     └────────────────────────────────────┘
```

**桌面客户端**：纯 UI 管理后台，打包后不含任何后端服务，通过 HTTP 连接云服务器。  
**云服务器**：运行 Axum API 服务 + PostgreSQL + Redis + RabbitMQ，处理所有业务逻辑。

---

## 角色体系

| 角色 | 说明 |
|------|------|
| **平台管理员** | 管理所有商户账号、套餐配置、查看全局统计数据 |
| **商户（上级）** | 注册后创建应用、生成卡密、查看激活记录，可创建下级代理 |
| **商户（代理）** | 使用邀请码加入上级，受配额限制生成卡密，获得分润统计 |
| **终端用户** | 通过商户软件内嵌 API 调用激活/验证卡密 |

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 桌面客户端 | [Tauri 2.0](https://tauri.app/)（纯前端壳，无内嵌服务） |
| 前端 UI | React 18 + TypeScript + Vite |
| 后端服务 | Rust + [Axum](https://github.com/tokio-rs/axum)（独立部署） |
| 数据库 | PostgreSQL + [SQLx](https://github.com/launchbadge/sqlx) |
| 缓存 | Redis（验证码存储、Rate Limiting、分布式锁，TTL 自动过期） |
| 消息队列 | RabbitMQ + [lapin](https://github.com/amqp-rs/lapin)（套餐异步降级/升级） |
| 认证 | JWT Access Token（2小时）+ Refresh Token（7天）无感续期，bcrypt 密码加密 |
| 数据加密 | AES-256-GCM 字段级加密 + SHA256 哈希索引，敏感字段（API Key / 邮箱 / 卡密 / 设备 ID）加密存储，哈希值用于快速查询 |
| 邮件 | [Lettre](https://lettre.rs/)（SMTP，支持 QQ/Gmail 等） |

---

## 功能特性

- **多商户隔离**：每个商户拥有独立的应用、卡密和数据，互不干扰
- **多级代理体系**：商户可生成邀请码邀请下级代理，支持配额划拨（限制代理生成卡密数量）、分润比例设置（统计每次激活的贡献）、启用/禁用/解除代理关系；代理可查看自己的上级信息和激活分润记录
- **套餐管理**：免费版/专业版，管理员可配置各套餐的应用数、卡密数、设备数限制
- **异步套餐降级**：专业版到期后通过 RabbitMQ 异步处理降级，Redis 分布式锁防并发，分批 UPDATE 防长事务，主进程不受影响
- **风控管理**：IP/设备黑名单（手动添加），异常激活告警（IP 频繁激活、设备多卡激活、异地激活），同一 IP 短时间内频率限制（防黄牛）
- **卡密前缀/格式自定义**：生成卡密时可指定前缀和格式
- **批量延期/缩短**：支持对选中卡密批量调整有效期
- **多设备支持**：每张卡密可配置最大绑定设备数（1~100 台）
- **联网验证**：软件每次启动调用 API 验证，服务端实时校验有效期和设备绑定
- **邮箱注册验证**：注册时发送 6 位数字验证码（Redis 存储，10 分钟有效，60 秒防刷）
- **批量生成卡密**：支持一次生成 1~1000 张，可设置有效期和设备数
- **卡密生命周期管理**：未使用 / 使用中 / 已过期 / 已禁用（可重新启用）
- **设备解绑**：商户可手动解绑指定设备，卡密恢复可用
- **统一登录**：管理员与商户使用同一登录入口，后端自动识别角色
- **接口分页**：商户管理、应用列表、卡密列表均支持分页
- **无感续期**：Access Token 2小时过期后自动用 Refresh Token 刷新，用户无需重新登录
- **Rate Limiting**：登录接口 IP 限速（10次/分钟），公开 API 限速（60次/分钟），基于 Redis 实现
- **WebSocket 实时推送**：商户端实时接收激活通知、站内信等事件
- **Webhook 支持**：应用可配置 Webhook URL，激活/验证成功时推送事件（HMAC-SHA256 签名）
- **站内信/公告**：管理员可发送全体公告或单独站内信，商户端实时收到未读提醒
- **字段级数据加密**：敏感字段（API Key、邮箱、卡密代码、设备 ID）采用 AES-256-GCM 加密存储，防止数据库泄露后敏感数据裸露
- **哈希索引查询**：使用 SHA256 哈希值建立唯一/普通索引，实现 O(1) 时间复杂度的加密字段查询，性能提升 100 倍+

---

## 部署

> 只需要服务器上装有 **Docker** 和 **Docker Compose**，无需 Rust、Node.js、PostgreSQL、Redis 等任何环境。

### 第一步：克隆代码

```bash
git clone https://github.com/zf26/kamism.git
cd kamism
```

### 第二步：配置环境变量

```bash
cp env.example .env
nano .env
```

`.env` 必填字段：

```env
POSTGRES_PASSWORD=强密码
RABBITMQ_PASSWORD=强密码
JWT_SECRET=随机32位以上字符串
ADMIN_EMAIL=admin@example.com
ADMIN_PASSWORD=Admin@123456
```

完整字段说明见 `env.example`。

---

### 方式一：单容器

所有服务（PostgreSQL + Redis + RabbitMQ + 后端 + Nginx 前端）打包进**一个容器**，只有一个 `kamism` 容器运行。

```bash
docker compose -f docker-compose.standalone.yml up -d --build
```

> 首次构建约需 **20~30 分钟**（需在镜像内安装 PostgreSQL、RabbitMQ 等），请耐心等待。

### 方式二：多容器（生产环境推荐）

各服务独立容器，便于维护、升级和故障排查，共 5 个容器。

```bash
docker compose up -d --build
```

> 首次构建约需 **10~20 分钟**。

---

### 访问

部署完成后通过以下地址访问（两种方式相同）：

| 地址 | 说明 |
|---|---|
| `http://your-server-ip:1420` | Web 管理控制台 |
| `http://your-server-ip:1420/api/` | 后端 REST API |
| `http://your-server-ip:1420/api/v1/activate` | 卡密激活接口（第三方软件调用） |

登录账号为 `.env` 中配置的 `ADMIN_EMAIL` / `ADMIN_PASSWORD`。

---

### 常用命令

```bash
# 查看运行状态（多容器）
docker compose ps

# 查看运行状态（单容器）
docker ps -f name=kamism

# 查看后端日志（多容器）
docker compose logs -f app

# 查看日志（单容器）
docker logs -f kamism

# 停止服务（多容器）
docker compose down

# 停止服务（单容器）
docker compose -f docker-compose.standalone.yml down

# 停止并清除数据（慎用）
docker compose down -v

# 更新代码后重新部署
git pull
docker compose up -d --build
# 或单容器
docker compose -f docker-compose.standalone.yml up -d --build
```

### 配置 HTTPS（推荐）

建议在服务器前置 Nginx 或使用宝塔面板配置 SSL 证书，反向代理到 `80` 端口：

```nginx
server {
    listen 443 ssl;
    server_name yourdomain.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location / {
        proxy_pass http://127.0.0.1:80;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### 打包桌面客户端（Tauri）

在本地开发机上执行（需要 Node.js + Rust + Tauri CLI）：

```bash
npm install
# 将 VITE_API_URL 指向你的服务器
echo 'VITE_API_URL=https://yourdomain.com/api' > .env.production
npm run tauri build
```

打包产物在 `src-tauri/target/release/bundle/`：
- Windows：`.msi` 安装包
- macOS：`.dmg`
- Linux：`.deb` / `.AppImage`

---

### 一、部署服务器端（手动，不使用 Docker）

```bash
# 1. 安装 Rust（服务器上执行）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 2. 上传源码到服务器（本机执行）
scp -r src-tauri/ server/ Cargo.toml user@your-server:/opt/kamism/

# 3. 服务器上编译
cd /opt/kamism
cargo build --release -p kamism-server

# 4. 创建数据库
psql -U postgres -c "CREATE DATABASE kamism;"

# 5. 创建 .env 配置文件
cat > /opt/kamism/.env << 'EOF'
DATABASE_URL=postgres://用户名:密码@localhost:5432/kamism
REDIS_URL=redis://127.0.0.1:6379
AMQP_URL=amqp://guest:guest@localhost:5672/%2f
RABBITMQ_PASSWORD=mq密码
JWT_SECRET=随机长密钥
PORT=9527
ADMIN_EMAIL=admin@yourdomain.com
ADMIN_PASSWORD=Admin@强密码
SMTP_HOST=smtp.qq.com
SMTP_PORT=465
SMTP_USER=your@qq.com
SMTP_PASS=授权码
SMTP_FROM_NAME=KamiSM
SMTP_FROM_EMAIL=your@qq.com
RUST_LOG=info
VITE_API_URL=https://yourdomain/api
API_URL=https://yourdomain/api
MASTER_KEY=xxx(64位16进制字符串)
EOF

# 6. 用 systemd 守护进程
sudo tee /etc/systemd/system/kamism.service << 'EOF'
[Unit]
Description=KamiSM Server
After=network.target postgresql.service redis.service rabbitmq-server.service

[Service]
Type=simple
WorkingDirectory=/opt/kamism
EnvironmentFile=/opt/kamism/.env
ExecStart=/opt/kamism/target/release/kamism-server
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable kamism
sudo systemctl start kamism
```

首次启动会自动执行数据库迁移并创建管理员账号。

### 二、部署前端（Nginx + 宝塔）

在同一域名下托管前端控制台和后端 API：

| 访问地址 | 服务 |
|---|:--|
| `https://yourdomain.com` | 前端控制台 |
| `https://yourdomain.com/api/` | Rust 后端（9527） |

**构建前端：**

```bash
# 前端控制台（在 .env.production 中配置 VITE_API_URL=https://yourdomain.com/api）
npm install
npm run build
# 上传 dist/ 到服务器 /www/wwwroot/yourdomain.com
```

**Nginx 关键配置：**

```nginx
# API 反向代理（去掉 /api 前缀转发给后端）
location /api/ {
    proxy_pass http://127.0.0.1:9527/;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 300s;
    proxy_buffering off;   # SSE 必须关闭缓冲
}

# 前端控制台（SPA fallback）
location / {
    alias /www/wwwroot/yourdomain.com/;
    try_files $uri $uri/ /kamism/index.html;
}
```

### 三、打包桌面客户端

在 `.env.production` 中配置服务器地址：

```env
VITE_API_URL=https://yourdomain.com/api
```

执行打包：

```bash
npm install
npm run tauri build
```

打包产物在 `src-tauri/target/release/bundle/`：
- Windows：`.msi` 安装包
- macOS：`.dmg`
- Linux：`.deb` / `.AppImage`

---

## 多级代理体系

商户之间可建立上下级代理关系，支持配额管理与分润统计。

### 核心功能

| 功能 | 说明 |
|------|------|
| 生成邀请码 | 上级商户生成 8 位唯一邀请码，设置初始配额和分润比例 |
| 加入关系 | 下级商户输入邀请码加入，一个商户只能有一个上级 |
| 配额管理 | 上级可随时增加/回收代理配额，配额用完后代理无法生成卡密 |
| 分润统计 | 每次卡密激活异步写入分润记录，上级可查看各代理贡献明细 |
| 启用/禁用 | 上级可禁用代理（禁用后配额校验不通过） |
| 解除关系 | 上级可彻底解除代理关系 |

### 数据库表

| 表名 | 说明 |
|------|------|
| `agent_relations` | 代理关系（含配额、分润比例、邀请码、状态） |
| `agent_quota_logs` | 配额调整日志（每次增减均有记录） |
| `agent_commission_logs` | 分润记录（每次激活异步写入） |

### API 接口

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/agent/invite` | 生成邀请码 |
| GET | `/agent/list` | 查看我的代理列表 |
| PATCH | `/agent/:id/quota` | 调整配额 |
| PATCH | `/agent/:id/commission` | 修改分润比例 |
| PATCH | `/agent/:id/status` | 启用/禁用代理 |
| DELETE | `/agent/:id` | 解除代理关系 |
| GET | `/agent/commissions` | 查看分润统计（我作为上级） |
| GET | `/agent/my` | 查看我的上级关系 |
| GET | `/agent/my/commissions` | 查看我的激活分润记录 |
| POST | `/agent/join/:code` | 使用邀请码加入上级 |

---

## 对外开放 API

供第三方软件集成，通过商户 `api_key` 鉴权，无需 JWT。

### 激活卡密

```http
POST https://yourdomain.com/api/v1/activate
Content-Type: application/json

{
  "api_key": "km_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
  "app_id": "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
  "card_code": "KAMI-XXXX-XXXX-XXXX-XXXX",
  "device_id": "设备唯一标识符",
  "device_name": "用户的电脑名称"
}
```

### 验证卡密（每次软件启动时调用）

```http
POST https://yourdomain.com/api/v1/verify
Content-Type: application/json

{
  "api_key": "km_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
  "app_id": "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
  "card_code": "KAMI-XXXX-XXXX-XXXX-XXXX",
  "device_id": "设备唯一标识符"
}
```

### 解绑设备

```http
POST https://yourdomain.com/api/v1/unbind
Content-Type: application/json

{
  "api_key": "km_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
  "app_id": "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
  "card_code": "KAMI-XXXX-XXXX-XXXX-XXXX",
  "device_id": "设备唯一标识符"
}
```

### 响应格式

```json
{
  "success": true,
  "valid": true,
  "message": "卡密有效",
  "data": {
    "card_code": "KAMI-XXXX-XXXX-XXXX-XXXX",
    "expires_at": "2025-01-01T00:00:00Z",
    "remaining_days": 30,
    "max_devices": 3,
    "current_devices": 1
  }
}
```

---

## 项目结构

```
kamism/
├── src/                        # 前端控制台 React
│   ├── components/Layout.tsx   # 侧边栏布局（含代理管理菜单）
│   ├── lib/api.ts              # API 请求封装（含 agentApi、blacklistApi 等）
│   ├── stores/auth.ts          # 认证状态（Zustand）
│   └── pages/
│       ├── auth/               # 登录、注册、重置密码
│       ├── admin/              # 平台总览、商户管理、套餐配置、消息管理
│       └── merchant/           # 控制台、应用、卡密、激活记录、消息中心
│           ├── Dashboard.tsx   # 数据总览
│           ├── Apps.tsx        # 应用管理
│           ├── Cards.tsx       # 卡密管理（含前缀/格式、批量延期）
│           ├── Activations.tsx # 激活记录
│           ├── Messages.tsx    # 消息中心
│           ├── Blacklist.tsx   # 风控管理（IP/设备黑名单、异常告警）
│           ├── Agents.tsx      # 代理管理（邀请码、配额、分润统计）
│           └── Settings.tsx    # 账号设置
├── src-tauri/                  # Tauri 桌面端 + Rust 后端
│   ├── migrations/             # 数据库迁移 SQL
│   │   ├── 001_init_complete.sql   # 基础表结构（商户、应用、卡密、激活、套餐、消息、Webhook）
│   │   ├── 002_perf_indexes.sql    # 性能优化复合索引
│   │   ├── 003_risk_control.sql    # 风控：IP/设备黑名单、异常告警
│   │   └── 004_agent_system.sql    # 多级代理：关系表、配额日志、分润记录
│   └── src/
│       ├── lib.rs              # Tauri 入口 + start_server()
│       ├── db/                 # 数据库连接池
│       ├── models/             # 数据模型
│       ├── routes/             # API 路由
│       │   ├── auth.rs         # 注册、登录、验证码、Token 刷新
│       │   ├── admin.rs        # 管理员接口
│       │   ├── apps.rs         # 应用管理
│       │   ├── cards.rs        # 卡密管理（含代理配额校验）
│       │   ├── activations.rs  # 激活记录
│       │   ├── merchant.rs     # 商户个人信息
│       │   ├── plan_config.rs  # 套餐配置
│       │   ├── blacklist.rs    # 风控管理
│       │   ├── agent.rs        # 代理体系（邀请、配额、分润）
│       │   ├── webhooks.rs     # Webhook 配置与推送
│       │   ├── messages.rs     # 站内信/公告
│       │   ├── health.rs       # 健康检查（DB/Redis/MQ 状态）
│       │   └── public_api.rs   # 对外 API（激活/验证/解绑，含分润写入）
│       ├── middleware/
│       │   ├── auth.rs         # JWT 中间件 + AppState
│       │   └── rate_limit.rs   # Rate Limiting
│       ├── workers/
│       │   └── downgrade.rs    # RabbitMQ 消费者（套餐降级/升级）
│       └── utils/
│           ├── jwt.rs          # JWT 生成与验证
│           ├── mq.rs           # RabbitMQ 工具
│           ├── ws.rs           # WebSocket 注册表与推送
│           ├── card_gen.rs     # 卡密/API Key 生成（含前缀/格式）
│           ├── kms.rs          # AES-256-GCM 加密管理
│           ├── mailer.rs       # SMTP 邮件发送
│           └── error.rs        # 统一错误处理
├── server/                     # 独立服务器 crate
├── Cargo.toml
├── .env.production
└── package.json
```

---

## 卡密格式

```
KAMI-XXXX-XXXX-XXXX-XXXX
```

使用大写字母和数字，去掉易混淆字符（`O`、`0`、`I`、`1`），共 16 位有效字符，随机生成。

---

## 数据加密

KamiSM 实现了**字段级 AES-256-GCM 加密 + SHA256 哈希索引**的双层安全方案，既保护敏感数据，又保证查询性能。

### 加密策略

| 表 | 字段 | 存储方式 | 用途 |
|---|---|---|---|
| merchants | api_key | `api_key_encrypted` (AES-256-GCM) | 加密存储 |
| merchants | api_key_hash | `api_key_hash` (SHA256) | 唯一索引查询 |
| merchants | email | `email_encrypted` (AES-256-GCM) | 加密存储 |
| merchants | email_hash | `email_hash` (SHA256) | 唯一索引查询 |
| cards | code | `code_encrypted` (AES-256-GCM) | 加密存储 |
| cards | code_hash | `code_hash` (SHA256) | 普通索引查询 |
| activations | device_id | `device_id_encrypted` (AES-256-GCM) | 加密存储 |
| activations | device_id_hash | `device_id_hash` (SHA256) | 普通索引查询 |

### 工作原理

1. **数据写入**：敏感字段使用 AES-256-GCM 加密后存储，同时计算 SHA256 哈希值用于索引
2. **数据查询**：通过哈希值在索引上快速定位记录（O(1) 时间复杂度），无需全表扫描和解密
3. **数据读取**：返回给前端前解密敏感字段，确保用户看到明文

### 性能对比

| 操作 | 之前（全表扫描） | 现在（哈希索引） | 性能提升 |
|------|---|---|---|
| 注册新商户 | O(n) + n 次解密 | O(1) 索引查询 | **100倍+** |
| 激活卡密 | O(n) + n 次解密 | O(1) 索引查询 | **100倍+** |
| 验证卡密 | O(m) 遍历 + m 次解密 | O(1) 索引查询 | **50倍+** |

### 安全性

- ✅ **数据库泄露防护**：敏感数据加密存储，哈希值单向不可逆
- ✅ **密钥管理**：主密钥 (MASTER_KEY) 独立管理，不存储在数据库
- ✅ **符合标准**：采用业界标准的 AES-256-GCM 和 SHA256 算法

### 快速配置

```bash
# 1. 生成主密钥（64位16进制字符串）
MASTER_KEY=$(openssl rand -hex 32)
echo "MASTER_KEY=$MASTER_KEY" >> .env

# 2. 数据库迁移会自动创建加密字段和哈希索引
# 迁移脚本：
#   - 008_remove_plaintext_fields.sql  # 删除明文字段
#   - 009_add_hash_indexes.sql         # 添加哈希索引
```

### 数据库架构

```sql
-- merchants 表
CREATE TABLE merchants (
    id UUID PRIMARY KEY,
    username VARCHAR(255) NOT NULL UNIQUE,
    password_hash VARCHAR(255) NOT NULL,
    api_key_encrypted TEXT NOT NULL,        -- AES-256-GCM 加密
    api_key_hash VARCHAR(64) NOT NULL UNIQUE,  -- SHA256 哈希（索引）
    email_encrypted TEXT NOT NULL,          -- AES-256-GCM 加密
    email_hash VARCHAR(64) NOT NULL UNIQUE, -- SHA256 哈希（索引）
    ...
);

-- cards 表
CREATE TABLE cards (
    id UUID PRIMARY KEY,
    code_encrypted TEXT NOT NULL,           -- AES-256-GCM 加密
    code_hash VARCHAR(64) NOT NULL,         -- SHA256 哈希（索引）
    ...
);

-- activations 表
CREATE TABLE activations (
    id UUID PRIMARY KEY,
    device_id_encrypted TEXT NOT NULL,      -- AES-256-GCM 加密
    device_id_hash VARCHAR(64) NOT NULL,    -- SHA256 哈希（索引）
    ...
);
```

---

## License

Copyright © 2026 KamiSM Contributors

This project is licensed under the **MIT License**.

完整协议文本见 [LICENSE](./LICENSE)。

---

## 赞赏支持

如果 KamiSM 对您有帮助，不妨请作者喝杯咖啡 ☕

感谢您的支持，这将激励作者持续维护和改进项目！

<div align="center">

| 微信支付 | 支付宝 |
|:---:|:---:|
| <img src="https://oss.fly-fly.fun/ext/wx.jpg" width="200"> | <img src="https://oss.fly-fly.fun/ext/zfb.jpg" width="200"> |

