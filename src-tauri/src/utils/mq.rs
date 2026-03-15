//! RabbitMQ 工具：连接、发布消息、消费消息
//! 消息体格式：JSON { merchant_id, action, issued_at(unix秒) }

use anyhow::Result;
use lapin::{
    options::*, types::FieldTable,
    BasicProperties, Channel, Connection, ConnectionProperties, Consumer,
};
use serde::{Deserialize, Serialize};

pub const DOWNGRADE_QUEUE: &str = "kamism.plan.downgrade";
pub const UPGRADE_QUEUE: &str = "kamism.plan.upgrade";

#[derive(Debug, Serialize, Deserialize)]
pub struct PlanMessage {
    pub merchant_id: String,
    pub action: String,   // "downgrade" | "upgrade"
    pub issued_at: i64,   // Unix 时间戳（秒），用于乱序校验
}

/// 建立 RabbitMQ 连接并返回 Channel（同时声明两个队列）
pub async fn connect(amqp_url: &str) -> Result<Channel> {
    let conn = Connection::connect(amqp_url, ConnectionProperties::default()).await?;
    let channel = conn.create_channel().await?;

    for queue in [DOWNGRADE_QUEUE, UPGRADE_QUEUE] {
        channel
            .queue_declare(
                queue.into(),
                QueueDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await?;
    }

    Ok(channel)
}

/// 发布降级消息
pub async fn publish_downgrade(channel: &Channel, merchant_id: &str) -> Result<()> {
    let msg = PlanMessage {
        merchant_id: merchant_id.to_string(),
        action: "downgrade".to_string(),
        issued_at: chrono::Utc::now().timestamp(),
    };
    publish(channel, DOWNGRADE_QUEUE, &serde_json::to_string(&msg)?).await
}

/// 发布升级恢复消息
pub async fn publish_upgrade(channel: &Channel, merchant_id: &str) -> Result<()> {
    let msg = PlanMessage {
        merchant_id: merchant_id.to_string(),
        action: "upgrade".to_string(),
        issued_at: chrono::Utc::now().timestamp(),
    };
    publish(channel, UPGRADE_QUEUE, &serde_json::to_string(&msg)?).await
}

async fn publish(channel: &Channel, queue: &str, payload: &str) -> Result<()> {
    channel
        .basic_publish(
            "".into(),
            queue.into(),
            BasicPublishOptions::default(),
            payload.as_bytes(),
            BasicProperties::default().with_delivery_mode(2),
        )
        .await?;
    Ok(())
}

/// 创建指定队列的消费者
pub async fn create_consumer(
    channel: &Channel,
    queue: &str,
    consumer_tag: &str,
) -> Result<Consumer> {
    let consumer = channel
        .basic_consume(
            queue.into(),
            consumer_tag.into(),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;
    Ok(consumer)
}
