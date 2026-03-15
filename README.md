<div align=center><img src="https://oss.fly-fly.fun/ext/kamism.png" width = "200" height = "200"></div>

# KamiSM — 卡密管理系统

> 基于 Tauri 2.0 + Rust (Axum) + React + PostgreSQL + Redis + RabbitMQ 构建的卡密即服务（KaaS）平台。

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
| **商户** | 注册后创建应用、生成卡密、查看激活记录，受免费/专业版限制 |
| **终端用户** | 通过商户软件内嵌 API 调用激活/验证卡密 |

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 桌面客户端 | [Tauri 2.0](https://tauri.app/)（纯前端壳，无内嵌服务） |
| 前端 UI | React 18 + TypeScript + Vite |
| 门户官网 | React + Vite（独立 `website/` 目录） |
| 后端服务 | Rust + [Axum](https://github.com/tokio-rs/axum)（独立部署） |
| 数据库 | PostgreSQL + [SQLx](https://github.com/launchbadge/sqlx) |
| 缓存 | Redis（验证码存储、Rate Limiting、分布式锁，TTL 自动过期） |
| 消息队列 | RabbitMQ + [lapin](https://github.com/amqp-rs/lapin)（套餐异步降级/升级） |
| 认证 | JWT Access Token（2小时）+ Refresh Token（7天）无感续期，bcrypt 密码加密 |
| 邮件 | [Lettre](https://lettre.rs/)（SMTP，支持 QQ/Gmail 等） |

---

## 功能特性

- **多商户隔离**：每个商户拥有独立的应用、卡密和数据，互不干扰
- **套餐管理**：免费版/专业版，管理员可配置各套餐的应用数、卡密数、设备数限制
- **异步套餐降级**：专业版到期后通过 RabbitMQ 异步处理降级，Redis 分布式锁防并发，分批 UPDATE 防长事务，主进程不受影响
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
- **SSE 流式响应**：支持实时推送

---

## 部署

### 前置要求

服务器需要：PostgreSQL、Redis、RabbitMQ、Rust 工具链。

```bash
# 安装 RabbitMQ（Ubuntu/Debian）
apt install rabbitmq-server
systemctl enable --now rabbitmq-server
```

### 一、部署服务器端

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

在同一域名下托管三个服务：

| 访问地址 | 服务 |
|---|---|
| `https://yourdomain.com/` | 门户官网 |
| `https://yourdomain.com/kamism/` | 前端控制台 |
| `https://yourdomain.com/api/` | Rust 后端（9527） |

**构建：**

```bash
# 前端控制台（在 .env.production 中配置 VITE_API_URL=https://yourdomain.com/api）
npm install
npm run build
# 上传 dist/ 到服务器 /www/wwwroot/yourdomain.com/kamism/

# 门户官网
cd website
npm install
npm run build
# 上传 website/dist/ 到服务器 /www/wwwroot/yourdomain.com/
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
location /kamism/ {
    alias /www/wwwroot/yourdomain.com/kamism/;
    try_files $uri $uri/ /kamism/index.html;
}

# 门户官网
location / {
    try_files $uri $uri/ /index.html;
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

## 对外开放 API

供第三方软件集成，通过商户 `api_key` 鉴权，无需 JWT。

### 激活卡密

```http
POST https://yourdomain.com/api/v1/activate
Content-Type: application/json

{
  "api_key": "km_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
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
│   ├── components/Layout.tsx   # 侧边栏布局
│   ├── lib/api.ts              # API 请求封装（VITE_API_URL 构建时注入）
│   ├── stores/auth.ts          # 认证状态（Zustand）
│   └── pages/
│       ├── auth/               # 登录、注册
│       ├── admin/              # 平台总览、商户管理、套餐配置
│       └── merchant/           # 控制台、应用、卡密、激活记录、设置
├── website/                    # 门户官网（独立 React 应用）
│   └── src/
│       ├── components/         # Nav、Footer 等组件
│       └── pages/              # 首页、功能介绍、下载等
├── src-tauri/                  # Tauri 桌面端 + Rust 后端
│   ├── migrations/             # 数据库迁移 SQL
│   └── src/
│       ├── lib.rs              # Tauri 入口 + start_server()（供 server/ 调用）
│       ├── db/                 # 数据库连接池
│       ├── models/             # 数据模型（app、card、merchant、admin、activation、plan_config）
│       ├── routes/             # API 路由
│       │   ├── auth.rs         # 注册、登录（限速）、发送验证码、Token 刷新
│       │   ├── admin.rs        # 管理员接口（商户管理、套餐设置）
│       │   ├── apps.rs         # 应用管理
│       │   ├── cards.rs        # 卡密管理
│       │   ├── activations.rs  # 激活记录
│       │   ├── merchant.rs     # 商户个人信息
│       │   ├── plan_config.rs  # 套餐配置管理
│       │   └── public_api.rs   # 对外 API（激活/验证/解绑）
│       ├── middleware/
│       │   ├── auth.rs         # JWT 中间件 + AppState
│       │   └── rate_limit.rs   # Rate Limiting（登录/公开API限速）
│       ├── workers/
│       │   └── downgrade.rs    # RabbitMQ 消费者（套餐降级/升级 Worker）
│       └── utils/
│           ├── jwt.rs          # JWT 生成与验证（Access Token + Refresh Token）
│           ├── mq.rs           # RabbitMQ 工具（连接、发布、消费，消息含时间戳）
│           ├── card_gen.rs     # 卡密/API Key 生成
│           ├── mailer.rs       # SMTP 邮件发送
│           └── error.rs        # 统一错误处理
├── server/                     # 独立服务器 crate（部署到云服务器）
│   ├── src/main.rs             # 调用 start_server()，无 Tauri 依赖
│   └── Cargo.toml
├── Cargo.toml                  # Workspace 根配置
├── .env.production             # 前端生产环境配置（VITE_API_URL）
└── package.json
```

---

## 卡密格式

```
KAMI-XXXX-XXXX-XXXX-XXXX
```

使用大写字母和数字，去掉易混淆字符（`O`、`0`、`I`、`1`），共 16 位有效字符，随机生成。

---

## License

MIT


### 