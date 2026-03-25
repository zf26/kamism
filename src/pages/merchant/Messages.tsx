import { useEffect, useState, useCallback } from 'react';
import { merchantMessagesApi } from '../../lib/api';
import { useWsEventStore } from '../../stores/wsEvent';
import { Bell, Pin, RefreshCw } from 'lucide-react';
import toast from 'react-hot-toast';

interface MsgItem {
  id: string;
  msg_type: string;
  title: string;
  content: string;
  target_type: string;
  pinned: boolean;
  is_read: boolean;
  created_at: string;
}

type Tab = 'messages' | 'notices';

export default function MerchantMessages() {
  const [tab, setTab] = useState<Tab>('messages');
  const [items, setItems] = useState<MsgItem[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<MsgItem | null>(null);
  const PAGE_SIZE = 15;

  const load = useCallback((p = 1) => {
    setLoading(true);
    const req = tab === 'messages'
      ? merchantMessagesApi.listMessages({ page: p, page_size: PAGE_SIZE })
      : merchantMessagesApi.listNotices({ page: p, page_size: PAGE_SIZE });
    req
      .then((res) => {
        if (res.data.success) {
          setItems(res.data.data);
          setTotal(res.data.total);
        }
      })
      .finally(() => setLoading(false));
  }, [tab]);

  useEffect(() => { setPage(1); load(1); }, [tab]);
  useEffect(() => { load(page); }, [page]);

  // 订阅事件总线：收到新消息时刷新列表（WS 连接由 Layout 统一维护）
  const lastEvent = useWsEventStore((s) => s.lastEvent);
  useEffect(() => {
    if (lastEvent?.event === 'new_message') {
      toast(<span>📬 新消息：<b>{String(lastEvent.data?.title ?? '')}</b></span>, { duration: 5000 });
      load(1);
      setPage(1);
    }
  }, [lastEvent]);

  const handleOpen = async (msg: MsgItem) => {
    setSelected(msg);
    if (tab === 'messages' && !msg.is_read) {
      try {
        await merchantMessagesApi.markRead(msg.id);
        setItems((prev) =>
          prev.map((m) => (m.id === msg.id ? { ...m, is_read: true } : m))
        );
      } catch { /* 已读失败静默 */ }
    }
  };

  const totalPages = Math.ceil(total / PAGE_SIZE);

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">消息中心</h1>
          <p className="page-subtitle">共 {total} 条</p>
        </div>
        <div className="page-header-actions">
          <button className="btn btn-ghost" onClick={() => load(page)}><RefreshCw size={14} /> 刷新</button>
        </div>
      </div>

      {/* Tab 切换 */}
      <div style={{ display: 'flex', gap: 4, marginBottom: 20, borderBottom: '1px solid var(--border)', paddingBottom: 0 }}>
        {(['messages', 'notices'] as Tab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            style={{
              padding: '8px 20px',
              background: 'none',
              border: 'none',
              borderBottom: tab === t ? '2px solid var(--accent)' : '2px solid transparent',
              color: tab === t ? 'var(--accent)' : 'var(--text-dim)',
              fontWeight: tab === t ? 700 : 500,
              fontSize: 14,
              cursor: 'pointer',
              transition: 'all 0.15s',
              marginBottom: -1,
            }}
          >
            {t === 'messages' ? '站内信' : '公告'}
          </button>
        ))}
      </div>

      {/* 消息列表 */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {loading ? (
          <div style={{ textAlign: 'center', padding: 60 }}><span className="spinner" /></div>
        ) : items.length === 0 ? (
          <div className="empty-state">
            <div className="empty-state-icon"><Bell size={36} style={{ opacity: 0.3 }} /></div>
            <div className="empty-state-text">暂无{tab === 'messages' ? '站内信' : '公告'}</div>
          </div>
        ) : items.map((m) => (
          <div
            key={m.id}
            onClick={() => handleOpen(m)}
            style={{
              background: 'var(--bg-card)',
              border: `1px solid ${selected?.id === m.id ? 'var(--accent)' : 'var(--border)'}`,
              borderRadius: 10,
              padding: '14px 18px',
              cursor: 'pointer',
              transition: 'all 0.15s',
              opacity: tab === 'messages' && !m.is_read ? 1 : 0.75,
            }}
            onMouseEnter={(e) => (e.currentTarget.style.borderColor = 'var(--border-light)')}
            onMouseLeave={(e) => (e.currentTarget.style.borderColor = selected?.id === m.id ? 'var(--accent)' : 'var(--border)')}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              {tab === 'messages' && !m.is_read && (
                <span style={{ width: 7, height: 7, borderRadius: '50%', background: 'var(--accent)', flexShrink: 0, display: 'inline-block' }} />
              )}
              {m.pinned && <Pin size={12} style={{ color: 'var(--accent)', flexShrink: 0 }} />}
              <span style={{
                fontWeight: (tab === 'messages' && !m.is_read) ? 700 : 500,
                color: 'var(--text)',
                fontSize: 14,
                flex: 1,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}>
                {m.title}
              </span>
              <span style={{ fontSize: 11, color: 'var(--text-muted)', flexShrink: 0 }}>
                {new Date(m.created_at).toLocaleDateString('zh-CN')}
              </span>
            </div>
          </div>
        ))}
      </div>

      {/* 分页 */}
      {totalPages > 1 && (
        <div className="pagination">
          <button className="page-btn" onClick={() => setPage((p) => Math.max(1, p - 1))} disabled={page === 1}>‹</button>
          {Array.from({ length: totalPages }, (_, i) => i + 1)
            .slice(Math.max(0, page - 3), Math.min(totalPages, page + 2))
            .map((p) => (
              <button key={p} className={`page-btn ${p === page ? 'active' : ''}`} onClick={() => setPage(p)}>{p}</button>
            ))}
          <button className="page-btn" onClick={() => setPage((p) => Math.min(totalPages, p + 1))} disabled={page >= totalPages}>›</button>
        </div>
      )}

      {/* 消息详情弹窗 */}
      {selected && (
        <div className="modal-overlay" onClick={() => setSelected(null)}>
          <div className="modal" style={{ maxWidth: 560 }} onClick={(e) => e.stopPropagation()}>
            <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', marginBottom: 16 }}>
              <h2 className="modal-title" style={{ margin: 0, fontSize: 17, flex: 1 }}>{selected.title}</h2>
              <button
                onClick={() => setSelected(null)}
                style={{ background: 'none', border: 'none', color: 'var(--text-muted)', cursor: 'pointer', padding: 4, fontSize: 18, lineHeight: 1 }}
              >✕</button>
            </div>
            <p style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 20 }}>
              {new Date(selected.created_at).toLocaleString('zh-CN')}
            </p>
            <div style={{
              color: 'var(--text-dim)',
              fontSize: 14,
              lineHeight: 1.8,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
            }}>
              {selected.content}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

