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

/// 单个 WS 连接的发送缓冲区容量。
///
/// ⚠️ 为什么必须有上限（这里曾经是 `unbounded_channel`）：
///
/// `unbounded_channel` 的 `send()` **永远不会失败**（除非接收端已 drop），
/// 这会连带来两个后果：
///
/// 1. **内存无限堆积**：消费端 `handle_ws` 的 task_a 是
///    `msg_rx.recv()` → `ws_tx.send().await` 写 socket。慢客户端
///    （网络差、或干脆不读）消费不过来时，消息只进不出地在 channel 里堆积，
///    直到把进程内存吃光。危害形态是**悬挂而非崩溃** —— 和本仓库
///    踩过的「连接池自耗」是同一类：堆栈里看不出是谁造成的。
/// 2. **死代码被伪装成活的**：下面的 `try_send` 失败分支（以及
///    `cleanup_dead`）在 unbounded 下**不可达**，所以「客户端断了要清理」
///    这件事从来没真正发生过，注册表只会越涨越大。
///
/// 取 256：按每条消息几百字节算，单连接最坏占用约几十 KB，
/// 足够吸收正常抖动，又能让小内存机器上几百个连接不至于失控。
const WS_CHANNEL_CAPACITY: usize = 256;

/// 单个 WS 连接的发送端（mpsc channel sender）
pub type WsSender = mpsc::Sender<Message>;

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

    /// 注册一个新连接，返回对应的接收端 + 本连接的发送端句柄。
    ///
    /// 返回 sender 是为了让 `handle_ws` 在断开时能调 [`Self::unregister`]
    /// 精确摘掉**这一个**连接（而不是靠 `cleanup_dead` 扫描）。同一个商户
    /// 可能开着多个标签页，不能按 merchant_id 整体删。
    ///
    /// ⚠️ 曾经这里不返回 sender，导致 `unregister` 无法被调用、
    /// 成了死代码，只能依赖 `cleanup_dead` —— 而 `cleanup_dead` 又因为
    /// unbounded channel 永不失败而从不触发。两个问题叠加起来，
    /// 就是「连接断开后注册表里还留着 sender」。
    pub async fn register(&self, merchant_id: Uuid) -> (mpsc::Receiver<Message>, WsSender) {
        let (tx, rx) = mpsc::channel(WS_CHANNEL_CAPACITY);
        let mut map = self.inner.write().await;
        map.entry(merchant_id).or_default().push(tx.clone());
        tracing::debug!(
            "[WS] 商户 {} 新增连接，当前连接数: {}",
            merchant_id,
            map.get(&merchant_id).map(|v| v.len()).unwrap_or(0)
        );
        (rx, tx)
    }

    /// 注销一个连接（连接生命周期结束时调用）
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

    /// 向指定商户的所有连接推送消息。
    ///
    /// 用 `try_send`（非阻塞）而不是 `send().await`：
    /// - 缓冲区满 → 说明该客户端消费不过来，视为死连接，断开它（下面统一清理）。
    ///   挂起等待只会把「慢客户端」的问题传染给广播方 —— 一个不读消息的
    ///   商户能拖住给所有人发公告的循环。
    /// - 通道关闭 → 客户端已断开。
    /// 两种情况都记入 `dead`，最后一次性清理，并留下日志（不静默）。
    pub async fn send_to(&self, merchant_id: &Uuid, msg: Message) {
        let map = self.inner.read().await;
        if let Some(senders) = map.get(merchant_id) {
            let mut dead = vec![];
            let mut slow = 0usize;
            for (i, tx) in senders.iter().enumerate() {
                match tx.try_send(msg.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // 消费不过来：丢弃本连接，避免消息在内存里无限堆积
                        slow += 1;
                        dead.push(i);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        dead.push(i);
                    }
                }
            }
            if slow > 0 {
                tracing::warn!(
                    "[WS] 商户 {} 有 {} 个连接消费过慢（缓冲区满），已断开以避免内存堆积",
                    merchant_id,
                    slow
                );
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
