# ─── 构建阶段 ───────────────────────────────────────────
FROM rust:1.89-slim AS builder

WORKDIR /app

# 安装系统依赖
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 先复制 Cargo 文件做依赖缓存层
COPY Cargo.toml Cargo.toml
COPY server/Cargo.toml server/Cargo.toml
COPY src-tauri/Cargo.toml src-tauri/Cargo.toml

# 创建占位源文件，让 cargo 先拉依赖
RUN mkdir -p server/src src-tauri/src \
    && echo 'fn main(){}' > server/src/main.rs \
    && echo 'pub fn run(){}' > src-tauri/src/lib.rs \
    && echo 'fn main(){}' > src-tauri/src/main.rs

RUN cargo build --release -p kamism-server 2>/dev/null || true

# 复制真实源码
COPY src-tauri/src src-tauri/src
COPY src-tauri/migrations src-tauri/migrations
COPY src-tauri/build.rs src-tauri/build.rs
COPY server/src server/src

# 删除占位编译缓存，强制重新编译
RUN touch src-tauri/src/lib.rs server/src/main.rs

# 正式构建
RUN cargo build --release -p kamism-server

# ─── 运行阶段 ───────────────────────────────────────────
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# 复制编译产物
COPY --from=builder /app/target/release/kamism-server /app/kamism-server

# 暴露端口
EXPOSE 9527

CMD ["/app/kamism-server"]

