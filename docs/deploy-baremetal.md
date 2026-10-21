# 传统部署（不用 Docker）

适用：Ubuntu 22.04 / 24.04，把后端编译成原生二进制、依赖用系统服务、Nginx 托管前端。
下文假设域名 `yourdomain.com`、安装目录 `/opt/kamism`、前端目录 `/var/www/kamism`。

## 0. 拓扑

```
浏览器 / 桌面端
      │ 443 (TLS)
      ▼
    Nginx  ── /            → /var/www/kamism （前端静态文件，SPA）
      │    └─ /api/        → 127.0.0.1:9527 （去掉 /api 前缀转发）
      ▼
 kamism-server  (Rust, axum, 监听 0.0.0.0:9527，只允许本机访问)
      ├── PostgreSQL  127.0.0.1:5432   kamism / kamism
      ├── Redis       127.0.0.1:6379
      └── RabbitMQ    127.0.0.1:5672   vhost `/`，用户 kamism
```

三个依赖**任一不可用后端就不启动**（Redis / MQ 在启动阶段强连接），所以顺序是「先起依赖，再起后端」。

## 1. 装系统依赖

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev git curl \
                    postgresql redis-server rabbitmq-server nginx
```

编译工具链（两件事都要：Rust ≥ 1.88、Node.js ≥ 20）：

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# Node 20（NodeSource）
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt install -y nodejs
```

> 后端全量 release 编译峰值内存约 **1.5 GB**。1 GB 的小机器先加 swap，否则会被 OOM 杀掉：
> `sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile`

## 2. 建系统用户与目录

```bash
sudo useradd --system --home /opt/kamism --shell /usr/sbin/nologin kamism
sudo mkdir -p /opt/kamism /var/www/kamism
sudo chown -R kamism:kamism /opt/kamism
```

## 3. PostgreSQL

不要用 `postgres` 超级用户跑业务库，建独立角色：

```bash
sudo -u postgres psql <<'SQL'
CREATE USER kamism WITH PASSWORD '换成强密码';
CREATE DATABASE kamism OWNER kamism;
SQL

# 自检：用业务账号能连上（Ubuntu 默认 host 段就是 scram-sha-256）
psql "postgres://kamism:你的数据库密码@127.0.0.1:5432/kamism" -c "select current_user, current_database();"
```

密码**只用字母数字**（放进 URL 里，带 `@ : / ?` 等字符需要百分号编码，容易配错）。

## 4. Redis

Ubuntu 默认就监听 `127.0.0.1:6379`、无密码，够用。要加密码：

```bash
sudo sed -i 's/^# requirepass .*/requirepass 换成强密码/' /etc/redis/redis.conf
sudo systemctl restart redis-server
# 对应 REDIS_URL=redis://:换成强密码@127.0.0.1:6379
```

## 5. RabbitMQ

```bash
sudo rabbitmqctl add_user kamism '换成强密码'
sudo rabbitmqctl set_permissions -p / kamism ".*" ".*" ".*"
sudo rabbitmqctl list_permissions -p /
```

vhost 是 `/`，写进 URL 必须转义成 `%2f`：
`AMQP_URL=amqp://kamism:密码@127.0.0.1:5672/%2f`

## 6. 编译后端

在**你自己登录的用户**下编译（`cargo` 在 `~/.cargo/bin`，别用 `sudo -u kamism` 跑，那个用户没有工具链）：

```bash
git clone https://github.com/zf26/kamism.git ~/kamism-src
cd ~/kamism-src
cargo build --release -p kamism-server
```

- 只需要 `Cargo.toml` / `Cargo.lock` / `server/` / `src-tauri/`，**不要拷 `target/` 和 `node_modules/`**（几百 MB，纯浪费时间）。
- 产物：`target/release/kamism-server`（Rust 部分静态链接，动态依赖只有 `libssl3` / `libc`）。
- 数据库迁移用 `sqlx::migrate!("./migrations")` **编译期内嵌**，投产时不需要带 `migrations/` 目录。
- 国内机器拉 crates.io 慢的话，先配镜像再编（`~/.cargo/config.toml` 里换 `[source.crates-io]` 的 `replace-with`）。

安装：

```bash
sudo install -m 755 target/release/kamism-server /opt/kamism/kamism-server
sudo chown kamism:kamism /opt/kamism/kamism-server
```

## 7. 编译前端

在服务器上编（也可以本机编好只传 `dist/`）：

```bash
cd ~/kamism-src
npm ci
VITE_BASE=/ VITE_API_URL=/api npm run build
sudo rsync -a --delete dist/ /var/www/kamism/
sudo chown -R www-data:www-data /var/www/kamism
```

要点：

- **`VITE_API_URL` 是编译期注入的**，改域名必须重新 build，重启 Nginx 没用。
- **`VITE_BASE` 决定静态资源 URL 的前缀**，必须和部署位置一致：本项目 Nginx 配置把前端放在站点根路径，
  所以这里写 `VITE_BASE=/`；若你放在 `https://yourdomain.com/kamism/` 这样的子路径，就改成 `VITE_BASE=/kamism/`。
  两者不一致时页面会白屏、控制台一堆 404，或者静态文件被 SPA 回落成 `index.html`
  （典型报错：`Unexpected token '<', "<!DOCTYPE "... is not valid JSON`）。
  构建时控制台会打印 `[vite] base = …`，发版前扫一眼就能确认。
  Windows PowerShell 不支持 `VAR=x cmd` 前置写法，用 `$env:VITE_BASE='/kamism/'; npm run build`，
  或直接跑内置脚本 `npm run build:kamism`（任意 shell 通用）。
- 路由 basename、登出跳转都从 `import.meta.env.BASE_URL` 推导，**只需要配 `VITE_BASE` 一处**，
  不用再改 `src/App.tsx`。子路径部署时 Nginx 要写 `location /kamism/ { try_files $uri $uri/ /kamism/index.html; }`，
  否则刷新子路由会 404。
- 仓库里的 `.env.production` 存的是别的域名，命令行前置的 `VITE_API_URL=/api` 优先级更高，会覆盖它。
  用相对路径 `/api` 的好处是同域、不担心 http/https 混用；写绝对地址 `https://yourdomain.com/api` 也行。
- **`rsync dist/` 要传全部内容**，不要只挑 `index.html` + `assets/`。`public/` 下的文件（如 `vite.svg`）同样在
  `dist/` 根目录，漏传时会被 SPA 回落成 HTML，排查起来很费劲。
- 中国地图数据（`china-map.json`）走 `import … ?url` 打进 assets，输出为 `assets/china-map-<hash>.json`，
  跟着 JS/CSS 一起部署。它以前放在 `public/` 里、代码写死 `fetch('/china-map.json')`，子路径部署时必然取不到。
- `VITE_SHIELD_URL` 是 APK 加固服务的独立地址（默认指向外部服务）。没部署加固服务就别配，或改成你自己的地址，否则相关页面会请求一个不存在的路径。

## 8. 写 `/opt/kamism/.env`

```bash
sudo -u kamism tee /opt/kamism/.env >/dev/null <<EOF
# ── 基础设施 ─────────────────────────────
DATABASE_URL=postgres://kamism:数据库密码@127.0.0.1:5432/kamism
REDIS_URL=redis://127.0.0.1:6379
AMQP_URL=amqp://kamism:MQ密码@127.0.0.1:5672/%2f
PORT=9527

# ── 密钥（必填，一次性生成，之后永不更换）──
JWT_SECRET=$(openssl rand -hex 32)
MASTER_KEY=$(openssl rand -hex 32)

# ── 初始管理员（仅库里没有管理员时创建）──
ADMIN_EMAIL=admin@yourdomain.com
ADMIN_PASSWORD=$(openssl rand -hex 16)

# ── 反向代理 ─────────────────────────────
# 同机 Nginx：只信任回环。配错的代价是「全站共用一个限流额度」+ IP 黑名单失效
TRUSTED_PROXIES=127.0.0.1
FRONTEND_URL=https://yourdomain.com
# 后端读它做 OAuth 回调兜底，填公开的 API 前缀
VITE_API_URL=https://yourdomain.com/api

# ── 可选 ─────────────────────────────────
RUST_LOG=info

# 邮件：不填 = 关闭邮件功能（不影响启动）。要发验证码/通知才填真值，
# 填错不会启动失败，但发信时会报连接/认证失败
# SMTP_HOST=smtp.qq.com
# SMTP_PORT=465
# SMTP_USER=your@qq.com
# SMTP_PASS=授权码
# SMTP_FROM_NAME=KamiSM
# SMTP_FROM_EMAIL=your@qq.com

# 支付：用哪个渠道填哪个，回调地址必须是公网可达的 /api 前缀
PAY_CHANNEL=mbdpay
MBD_APP_ID=
MBD_APP_KEY=
MBD_NOTIFY_URL=https://yourdomain.com/api/pay/notify
EOF

sudo chmod 600 /opt/kamism/.env
```

三个坑，全部是**启动即失败**（刻意设计，别去绕过）：

| 变量 | 规则 |
|---|---|
| `JWT_SECRET` | ≥ 32 位、≥ 8 种不同字符，且不能含 `change_me` / `example` / `todo` 等占位符 |
| `MASTER_KEY` | 必须是 **64 个十六进制字符**（32 字节）。丢了就是所有加密字段（API Key、邮箱、卡密、设备 ID）永久读不出来 |
| `ADMIN_PASSWORD` | ≥ 12 位、≥ 3 类字符（或 ≥ 24 位），且不能是 `Admin@123456` 这类公开口令、不能等于邮箱前缀 |

`Admin@强密码` 只有 9 位，会被拒绝——直接用 `openssl rand -hex 16`。
管理员密码只在「库里一个管理员都没有」时生效，建好之后改的是数据库里的哈希（管理端「系统设置」可改密）。

## 9. systemd

```bash
sudo tee /etc/systemd/system/kamism.service >/dev/null <<'EOF'
[Unit]
Description=KamiSM Backend
After=network-online.target postgresql.service redis-server.service rabbitmq-server.service
Wants=network-online.target

[Service]
Type=simple
User=kamism
Group=kamism
WorkingDirectory=/opt/kamism
EnvironmentFile=/opt/kamism/.env
ExecStart=/opt/kamism/kamism-server
# release 构建用的是 panic=abort：任何 panic 都会让进程直接退出，
# 所以这里必须是 always，而不是 on-failure（两者都覆盖 SIGABRT，但 always 更省心）
Restart=always
RestartSec=3
LimitNOFILE=65535
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now kamism
sudo systemctl status kamism --no-pager
```

首次启动会自动跑迁移、建管理员。看日志确认三行关键输出：

```bash
sudo journalctl -u kamism -n 50 --no-pager
# 数据库迁移完成
# 客户端 IP 策略: 已信任 1 条代理网段（TRUSTED_PROXIES）（TRUSTED_PROXIES="127.0.0.1"）
# KamiSM 服务器已启动，监听端口: 9527
```

## 10. Nginx + HTTPS

`/etc/nginx/conf.d/kamism.conf`（Debian/Ubuntu 的 `conf.d/*.conf` 被包含在 `http` 上下文里，`map` 可以写在文件顶层）。
**先只写 80 端口这一段**，跑通再让 certbot 加 TLS —— 反过来先写 `ssl_certificate` 指向还不存在的证书，`nginx -t` 会直接失败：

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    listen 80;
    server_name yourdomain.com;

    root  /var/www/kamism;
    index index.html;

    add_header X-Content-Type-Options nosniff always;
    add_header Referrer-Policy strict-origin-when-cross-origin always;
    add_header X-Frame-Options SAMEORIGIN always;

    # SPA：找不到的路径回落到 index.html
    location / {
        try_files $uri $uri/ /index.html;
    }

    # 带哈希的构建产物，长缓存
    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
        try_files $uri =404;
    }

    # 后端 API：注意 proxy_pass 结尾的 / 会去掉 /api 前缀（后端路由本身没有 /api）
    location /api/ {
        proxy_pass http://127.0.0.1:9527/;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 实时推送（/api/ws/messages）必须的两个头
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;

        # 后端本身放宽到 100MB，Nginx 默认只有 1MB，大请求会 413
        client_max_body_size 100m;
    }
}
```

```bash
sudo nginx -t && sudo systemctl reload nginx
curl -sI http://yourdomain.com/ | head -1        # 200 说明前端目录和 SPA 回落没问题

# 证书：--nginx 会复制一份 server 块到 443、加上 ssl_certificate 并做 80→443 跳转
sudo apt install -y certbot python3-certbot-nginx
sudo certbot --nginx -d yourdomain.com
```

防火墙只放 80/443，**不要**把 9527 暴露到公网：

```bash
sudo ufw allow 'Nginx Full'
sudo ufw enable
```

## 11. 验收清单

```bash
# 1. 后端直连
curl -s http://127.0.0.1:9527/health
# {"status":"ok","db":"ok","redis":"ok","mq":"ok","uptime_secs":...}
# 任一依赖挂掉会返回 503 + "degraded"

# 2. 经 Nginx
curl -s https://yourdomain.com/api/health

# 3. 前端能否加载
curl -sI https://yourdomain.com/ | head -1        # 200

# 4. 真实 IP 是否被识别（关键）
#    登录一次，再看管理端「审计日志」里的 IP 是否是你的公网 IP。
#    如果是 127.0.0.1，说明 TRUSTED_PROXIES 没生效（或 Nginx 没传 X-Real-IP）。
```

再手工确认两件事：浏览器能登录；商户端能收到实时推送（激活一条卡密，页面右上角消息不刷新 = WebSocket 没通，回去查 `Upgrade` / `Connection` 两个头）。

## 12. 升级

```bash
cd ~/kamism-src
git pull
cargo build --release -p kamism-server
sudo install -m 755 target/release/kamism-server /opt/kamism/kamism-server
sudo systemctl restart kamism
sudo journalctl -u kamism -n 30 --no-pager     # 确认「数据库迁移完成」，无报错

# 前端有改动时
VITE_BASE=/ VITE_API_URL=/api npm run build
sudo rsync -a --delete dist/ /var/www/kamism/
sudo chown -R www-data:www-data /var/www/kamism
```

迁移是**启动时自动执行**的，失败会直接退出（不会带着半套 schema 跑），看 journal 即可。
回滚：把旧二进制放回去 + `systemctl restart`。注意**迁移不回滚**，跨版本回退前先 `pg_dump`。

## 13. 备份

两样东西必须一起备份，缺一个都等于没备份：

```bash
sudo install -d -o postgres -g postgres /var/backups/kamism

# 库（-Fc 自定义格式，支持并行恢复）
sudo -u postgres pg_dump -Fc kamism -f /var/backups/kamism/kamism-$(date +%F).dump

# 配置（MASTER_KEY 在里面，权限 600）
sudo tar czf /var/backups/kamism/kamism-env-$(date +%F).tgz -C /opt/kamism .env
sudo chmod 600 /var/backups/kamism/*
```

恢复：

```bash
sudo -u postgres pg_restore -d kamism --clean --if-exists /var/backups/kamism/kamism-2026-10-21.dump
```

`.env` 里的 `MASTER_KEY` 丢了，数据库备份里那些 `*_encrypted` 字段就再也解不开（API Key、邮箱、卡密、设备 ID 全部报废）。

## 14. 排错对照

| 现象 | 原因 |
|---|---|
| 启动即退出，`JWT_SECRET 不可用` | 用了占位符 / 长度 < 32 / 字符种类 < 8 |
| `ADMIN_PASSWORD ... 少于 12 位` | 示例口令太短，用 `openssl rand -hex 16` |
| `MASTER_KEY 长度错误` | 必须是 64 个 hex 字符，不是随便一串 |
| `RabbitMQ 连接失败 ... ACCESS_REFUSED` | 用户/vhost 权限不对；或 URL 里 `/` 忘了转义成 `%2f` |
| `数据库连接失败` | 密码含特殊字符没编码、`pg_hba.conf` 不允许该网段、库 owner 不对 |
| 所有人共用一个限流额度 / 黑名单不生效 | `TRUSTED_PROXIES` 没含 `127.0.0.1`，或 Nginx 没传 `X-Real-IP` |
| 实时推送连不上（浏览器报握手失败） | Nginx 缺 `Upgrade` / `Connection`，或 `proxy_buffering on` |
| 上传大文件 413 | `client_max_body_size` 太小 |
| 前端刷新子路由 404 | 少了 `try_files ... /index.html` |
| 接口全 502 | 后端没起、端口不是 9527、或 `proxy_pass` 少写了结尾 `/` 导致路径变成 `/api/auth/...` |

## 15. 和 Docker 版的差异

| 项 | Docker | 传统部署 |
|---|---|---|
| `TRUSTED_PROXIES` | `172.28.0.0/16`（compose 钉死的容器网段） | `127.0.0.1`（同机 Nginx） |
| 进程管理 | supervisord | systemd |
| 依赖版本 | 镜像内固定（PG15 / Redis / RabbitMQ） | 跟发行版走，升级前先看迁移是否兼容 |
| 迁移文件 | 打进镜像 | 编译期内嵌，无需额外拷贝 |
| 对外端口 | 1420（容器内 Nginx） | 443（宿主机 Nginx），9527 仅本机 |
