import { useEffect, useState } from 'react';
import { adminMessagesApi } from '../../lib/api';
import { Plus, Trash2, RefreshCw, Pin } from 'lucide-react';
import { useConfirm } from '../../stores/confirm';
import toast from 'react-hot-toast';

interface Message {
  id: string;
  msg_type: string;
  title: string;
  content: string;
  target_type: string;
  target_id: string | null;
  pinned: boolean;
  read_count: number;
  created_at: string;
}

interface SendForm {
  msg_type: string;
  title: string;
  content: string;
  target_type: string;
  target_email: string;
  pinned: boolean;
  expires_at: string;
}

const defaultForm: SendForm = {
  msg_type: 'notice',
  title: '',
  content: '',
  target_type: 'all',
  target_email: '',
  pinned: false,
  expires_at: '',
};

export default function AdminMessages() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [form, setForm] = useState<SendForm>(defaultForm);
  const [filterType, setFilterType] = useState('');
  const PAGE_SIZE = 15;

  const load = (p = page) => {
    setLoading(true);
    adminMessagesApi
      .list({ page: p, page_size: PAGE_SIZE, msg_type: filterType || undefined })
      .then((res) => {
        if (res.data.success) {
          setMessages(res.data.data);
          setTotal(res.data.total);
        }
      })
      .finally(() => setLoading(false));
  };

  useEffect(() => { load(1); setPage(1); }, [filterType]);
  useEffect(() => { load(page); }, [page]);

  const handleSend = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.title.trim()) { toast.error('标题不能为空'); return; }
    if (!form.content.trim()) { toast.error('内容不能为空'); return; }
    if (form.msg_type === 'message' && form.target_type === 'single' && !form.target_email.trim()) {
      toast.error('单发消息必须填写商户邮箱'); return;
    }
    setSubmitting(true);
    try {
      const res = await adminMessagesApi.send({
        msg_type: form.msg_type,
        title: form.title,
        content: form.content,
        target_type: form.msg_type === 'notice' ? 'all' : form.target_type,
        target_email: form.target_type === 'single' ? form.target_email : undefined,
        pinned: form.pinned,
        expires_at: form.expires_at || undefined,
      });
      if (res.data.success) {
        toast.success('发送成功');
        setShowModal(false);
        setForm(defaultForm);
        load(1); setPage(1);
      } else {
        toast.error(res.data.message);
      }
    } catch { toast.error('发送失败'); }
    finally { setSubmitting(false); }
  };

  const confirm = useConfirm();

  const handleDelete = async (id: string) => {
    const ok = await confirm({ title: '删除消息', message: '确认删除该消息？此操作不可撤销。', confirmText: '删除', danger: true });
    if (!ok) return;
    try {
      const res = await adminMessagesApi.delete(id);
      if (res.data.success) { toast.success('删除成功'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('删除失败'); }
  };

  const handleTogglePin = async (msg: Message) => {
    try {
      const res = await adminMessagesApi.update(msg.id, { pinned: !msg.pinned });
      if (res.data.success) { toast.success(msg.pinned ? '已取消置顶' : '已置顶'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  const totalPages = Math.ceil(total / PAGE_SIZE);
  const typeLabel = (t: string) => t === 'notice' ? '公告' : '站内信';
  const targetLabel = (m: Message) => m.target_type === 'all' ? '全体' : `单发`;

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">消息管理</h1>
          <p className="page-subtitle">共 {total} 条消息</p>
        </div>
        <div className="page-header-actions">
          <select
            value={filterType}
            onChange={(e) => setFilterType(e.target.value)}
            style={{ width: 'auto', fontSize: 13 }}
          >
            <option value="">全部类型</option>
            <option value="notice">公告</option>
            <option value="message">站内信</option>
          </select>
          <button className="btn btn-ghost" onClick={() => load()}><RefreshCw size={14} /> 刷新</button>
          <button className="btn btn-primary" onClick={() => setShowModal(true)}><Plus size={15} /> 发送消息</button>
        </div>
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr>
            <th>类型</th><th>标题</th><th>收件方</th><th>置顶</th><th>已读</th><th>发送时间</th><th>操作</th>
          </tr></thead>
          <tbody>
            {loading ? (
              <tr><td colSpan={7} style={{ textAlign: 'center', padding: 40 }}><span className="spinner" /></td></tr>
            ) : messages.length === 0 ? (
              <tr><td colSpan={7}>
                <div className="empty-state"><div className="empty-state-icon">📭</div><div className="empty-state-text">暂无消息</div></div>
              </td></tr>
            ) : messages.map((m) => (
              <tr key={m.id}>
                <td>
                  <span className={`badge ${m.msg_type === 'notice' ? 'badge-active' : 'badge-unused'}`}>
                    {typeLabel(m.msg_type)}
                  </span>
                </td>
                <td style={{ maxWidth: 260, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: 'var(--text)' }}>
                  {m.pinned && <Pin size={12} style={{ color: 'var(--accent)', marginRight: 4, display: 'inline' }} />}
                  {m.title}
                </td>
                <td>{targetLabel(m)}</td>
                <td>{m.pinned ? <span style={{ color: 'var(--accent)' }}>是</span> : '—'}</td>
                <td>{m.read_count}</td>
                <td>{new Date(m.created_at).toLocaleString('zh-CN')}</td>
                <td>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <button
                      className="btn btn-sm btn-ghost"
                      style={{ color: m.pinned ? 'var(--warning)' : 'var(--text-dim)' }}
                      onClick={() => handleTogglePin(m)}
                      title={m.pinned ? '取消置顶' : '置顶'}
                    ><Pin size={12} /></button>
                    <button className="btn btn-sm btn-danger" onClick={() => handleDelete(m.id)}><Trash2 size={12} /></button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

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

      {showModal && (
        <div className="modal-overlay" onClick={() => setShowModal(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2 className="modal-title">发送消息</h2>
            <form onSubmit={handleSend}>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
                <div className="form-group">
                  <label className="form-label">消息类型</label>
                  <select value={form.msg_type} onChange={(e) => setForm({ ...form, msg_type: e.target.value })} >
                    <option value="notice">公告</option>
                    <option value="message">站内信</option>
                  </select>
                </div>
                {form.msg_type === 'message' && (
                  <div className="form-group">
                    <label className="form-label">收件范围</label>
                    <select value={form.target_type} onChange={(e) => setForm({ ...form, target_type: e.target.value })}>
                      <option value="all">全体商户</option>
                      <option value="single">指定商户</option>
                    </select>
                  </div>
                )}
              </div>
              {form.msg_type === 'message' && form.target_type === 'single' && (
                <div className="form-group">
                  <label className="form-label">商户邮箱</label>
                  <input
                    type="email"
                    value={form.target_email}
                    onChange={(e) => setForm({ ...form, target_email: e.target.value })}
                    placeholder="merchant@example.com"
                    required
                  />
                </div>
              )}
              <div className="form-group">
                <label className="form-label">标题</label>
                <input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} placeholder="消息标题" required />
              </div>
              <div className="form-group">
                <label className="form-label">内容</label>
                <textarea
                  value={form.content}
                  onChange={(e) => setForm({ ...form, content: e.target.value })}
                  placeholder="消息正文..."
                  rows={5}
                  required
                  style={{ resize: 'vertical' }}
                />
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
                <div className="form-group">
                  <label className="form-label">到期时间（可选）</label>
                  <input type="datetime-local" value={form.expires_at} onChange={(e) => setForm({ ...form, expires_at: e.target.value })} />
                </div>
                <div className="form-group" style={{ display: 'flex', alignItems: 'flex-end', paddingBottom: 2 }}>
                  <label style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', fontSize: 13, color: 'var(--text-dim)' }}>
                    <input
                      type="checkbox"
                      checked={form.pinned}
                      onChange={(e) => setForm({ ...form, pinned: e.target.checked })}
                      style={{ width: 'auto', accentColor: 'var(--accent)' }}
                    />
                    置顶显示
                  </label>
                </div>
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => { setShowModal(false); setForm(defaultForm); }}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting}>
                  {submitting ? <span className="spinner" /> : '发送'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}

