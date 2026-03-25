//! WebSocket 连接注册表
//!
//! 维护一张 merchant_id → 多个 WS sender 的映射，支持：
//! - 同一商户多标签页同时在线
//! - 向指定商户或全体商户广播消息
//! - 连接断开时自动清理

use axum::extract::ws::Message;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// 单个 WS 连接的发送端（mpsc channel sender）
pub type WsSender = mpsc::UnboundedSender<Message>;

/// 连接注册表：merchant_id → [WsSender, ...]
///
/// Arc<RwLock> 保证跨线程安全；读多写少场景下 RwLock 比 Mutex 更高效
#[derive(Clone, Default)]
pub struct WsRegistry {
    inner: Arc<RwLock<HashMap<Uuid, Vec<WsSender>>>>,
}

impl WsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 注册一个新连接，返回对应的接收端
    pub async fn register(&self, merchant_id: Uuid) -> mpsc::UnboundedReceiver<Message> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut map = self.inner.write().await;
        map.entry(merchant_id).or_default().push(tx);
        tracing::debug!("[WS] 商户 {} 新增连接，当前连接数: {}",
            merchant_id,
            map.get(&merchant_id).map(|v| v.len()).unwrap_or(0)
        );
        rx
    }

    /// 注销一个连接（发送端关闭后调用）
    pub async fn unregister(&self, merchant_id: Uuid, sender: &WsSender) {
        let mut map = self.inner.write().await;
        if let Some(senders) = map.get_mut(&merchant_id) {
            // 用指针地址比较找到对应 sender 并移除
            senders.retain(|s| !s.same_channel(sender));
            if senders.is_empty() {
                map.remove(&merchant_id);
            }
        }
        tracing::debug!("[WS] 商户 {} 连接断开", merchant_id);
    }

    /// 向指定商户的所有连接推送消息
    pub async fn send_to(&self, merchant_id: &Uuid, msg: Message) {
        let map = self.inner.read().await;
        if let Some(senders) = map.get(merchant_id) {
            let mut dead = vec![];
            for (i, tx) in senders.iter().enumerate() {
                if tx.send(msg.clone()).is_err() {
                    dead.push(i);
                }
            }
            if !dead.is_empty() {
                drop(map);
                self.cleanup_dead(merchant_id).await;
            }
        }
    }

    /// 向全体在线商户广播消息
    pub async fn broadcast(&self, msg: Message) {
        let map = self.inner.read().await;
        let merchant_ids: Vec<Uuid> = map.keys().cloned().collect();
        drop(map);
        for mid in merchant_ids {
            self.send_to(&mid, msg.clone()).await;
        }
    }

    /// 清理某商户已断开的连接（公开，供路由层调用）
    pub async fn cleanup_dead_pub(&self, merchant_id: Uuid) {
        self.cleanup_dead(&merchant_id).await;
    }

    /// 清理某商户已断开的连接
    async fn cleanup_dead(&self, merchant_id: &Uuid) {
        let mut map = self.inner.write().await;
        if let Some(senders) = map.get_mut(merchant_id) {
            senders.retain(|tx| !tx.is_closed());
            if senders.is_empty() {
                map.remove(merchant_id);
            }
        }
    }

    /// 获取当前在线商户数（用于监控）
    pub async fn online_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

