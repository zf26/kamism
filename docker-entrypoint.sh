#!/bin/bash
# KamiSM 单容器初始化脚本
set -e

export RABBITMQ_NODENAME=rabbit@localhost

# ─── 初始化 PostgreSQL 数据目录（仅首次）────────────
if [ ! -f /data/postgres/PG_VERSION ]; then
    echo "==> [init] 初始化 PostgreSQL 数据目录..."
    mkdir -p /data/postgres
    chown postgres:postgres /data/postgres
    su - postgres -c "/usr/lib/postgresql/15/bin/initdb -D /data/postgres"
    echo "local all all trust" > /data/postgres/pg_hba.conf
    echo "host all all 127.0.0.1/32 md5" >> /data/postgres/pg_hba.conf
    echo "host all all ::1/128 md5" >> /data/postgres/pg_hba.conf
    echo "==> [init] PostgreSQL 数据目录初始化完成"
fi

# ─── 等待 PostgreSQL 启动 ────────────────────────────
echo "==> [init] 等待 PostgreSQL 启动..."
until pg_isready -U postgres -q 2>/dev/null; do
    sleep 1
done
echo "==> [init] PostgreSQL 已就绪"

# ─── 创建数据库和用户（仅首次）──────────────────────
# 给 postgres 超级用户设置密码（后端默认用 postgres 用户连接）
psql -U postgres -c "ALTER USER postgres WITH PASSWORD '${POSTGRES_PASSWORD}'"

psql -U postgres -tc "SELECT 1 FROM pg_roles WHERE rolname='kamism'" | grep -q 1 || \
    psql -U postgres -c "CREATE USER kamism WITH PASSWORD '${POSTGRES_PASSWORD}'"

psql -U postgres -tc "SELECT 1 FROM pg_database WHERE datname='kamism'" | grep -q 1 || \
    psql -U postgres -c "CREATE DATABASE kamism OWNER kamism"

# ─── 等待 RabbitMQ 启动 ──────────────────────────────
echo "==> [init] 等待 RabbitMQ 启动..."
for i in $(seq 1 30); do
    rabbitmqctl -n rabbit@localhost status >/dev/null 2>&1 && break
    echo "==> [init] 等待 RabbitMQ... ($i/30)"
    sleep 3
done
echo "==> [init] RabbitMQ 已就绪"

# ─── 创建 RabbitMQ 用户（如果不存在）────────────────
rabbitmqctl -n rabbit@localhost list_users 2>/dev/null | grep -q kamism || \
    rabbitmqctl -n rabbit@localhost add_user kamism "${RABBITMQ_PASSWORD}"
rabbitmqctl -n rabbit@localhost set_permissions -p / kamism ".*" ".*" ".*" 2>/dev/null || true

# ─── 启动 KamiSM 后端 ────────────────────────────────
echo "==> [init] 初始化完成，启动 KamiSM 后端..."
supervisorctl -c /etc/supervisor/conf.d/kamism.conf start kamism
echo "==> [init] KamiSM 已启动！访问 http://your-server-ip:1420"
