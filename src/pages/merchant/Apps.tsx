import { useEffect, useState } from 'react';
import { appsApi } from '../../lib/api';
import { Plus, Trash2, RefreshCw, Power, Copy } from 'lucide-react';
import toast from 'react-hot-toast';
import { useConfirm } from '../../stores/confirm';

interface App {
  id: string;
  app_name: string;
  description: string | null;
  status: string;
  created_at: string;
}

const PAGE_SIZE_OPTIONS = [5, 10, 15, 20];

export default function Apps() {
  const [apps, setApps] = useState<App[]>([]);
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [total, setTotal] = useState(0);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({ app_name: '', description: '' });
  const [submitting, setSubmitting] = useState(false);

  const load = (p = page, ps = pageSize) => {
    setLoading(true);
    appsApi.list({ page: p, page_size: ps }).then(res => {
      if (res.data.success) {
        setApps(res.data.data);
        setTotal(res.data.total ?? res.data.data.length);
      }
    }).finally(() => setLoading(false));
  };

  const handlePageSize = (ps: number) => { setPageSize(ps); setPage(1); };

  useEffect(() => { load(page, pageSize); }, [page, pageSize]);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    try {
      const res = await appsApi.create({ app_name: form.app_name, description: form.description || undefined });
      if (res.data.success) {
        toast.success('应用创建成功');
        setShowModal(false);
        setForm({ app_name: '', description: '' });
        setPage(1); load(1, pageSize);
      } else {
        toast.error(res.data.message);
      }
    } catch { toast.error('创建失败'); }
    finally { setSubmitting(false); }
  };

  const confirm = useConfirm();

  const handleDelete = async (id: string) => {
    const ok = await confirm({ title: '删除应用', message: '确认删除此应用？关联的卡密也将被一并删除，此操作不可撤销。', confirmText: '删除', danger: true });
    if (!ok) return;
    try {
      const res = await appsApi.delete(id);
      if (res.data.success) { toast.success('删除成功'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('删除失败'); }
  };

  const handleToggle = async (id: string, status: string) => {
    const next = status === 'active' ? 'disabled' : 'active';
    try {
      const res = await appsApi.updateStatus(id, next);
      if (res.data.success) { toast.success('状态已更新'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">我的应用</h1>
          <p className="page-subtitle">共 {total} 个应用</p>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => load(page)}><RefreshCw size={14} /> 刷新</button>
          <button className="btn btn-primary" onClick={() => setShowModal(true)}><Plus size={15} /> 新建应用</button>
        </div>
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr><th>应用名称</th><th>描述</th><th>ID</th><th>状态</th><th>创建时间</th><th>操作</th></tr></thead>
          <tbody>
            {loading ? (
              <tr><td colSpan={6} style={{ textAlign: 'center', padding: 40 }}><span className="spinner" /></td></tr>
            ) : apps.length === 0 ? (
              <tr><td colSpan={6}><div className="empty-state"><div className="empty-state-icon">📦</div><div className="empty-state-text">暂无应用，点击「新建应用」开始</div></div></td></tr>
            ) : apps.map(app => (
              <tr key={app.id}>
                <td><span style={{ color: 'var(--text)', fontWeight: 600 }}>{app.app_name}</span></td>
                <td><span style={{ color: 'var(--text-muted)' }}>{app.description || '—'}</span></td>
                <td>
                  <span
                    className="mono"
                    style={{ fontSize: 11, cursor: 'pointer', display: 'inline-flex', alignItems: 'center', gap: 4, color: 'var(--text-muted)' }}
                    title="点击复制完整 ID"
                    onClick={() => { navigator.clipboard.writeText(app.id); toast.success('ID 已复制'); }}
                  >
                    {app.id.slice(0, 8)}…<Copy size={10} />
                  </span>
                </td>
                <td><span className={`badge badge-${app.status}`}>{app.status === 'active' ? '正常' : '禁用'}</span></td>
                <td>{new Date(app.created_at).toLocaleDateString('zh-CN')}</td>
                <td>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <button className="btn btn-sm btn-ghost" onClick={() => handleToggle(app.id, app.status)}><Power size={12} /></button>
                    <button className="btn btn-sm btn-danger" onClick={() => handleDelete(app.id)}><Trash2 size={12} /></button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="pagination">
        <button className="page-btn" onClick={() => setPage(p => Math.max(1, p - 1))} disabled={page === 1}>‹</button>
        {Array.from({ length: Math.ceil(total / pageSize) }, (_, i) => i + 1)
          .slice(Math.max(0, page - 3), Math.min(Math.ceil(total / pageSize), page + 2))
          .map(p => (
            <button key={p} className={`page-btn ${p === page ? 'active' : ''}`} onClick={() => setPage(p)}>{p}</button>
          ))}
        <button className="page-btn" onClick={() => setPage(p => Math.min(Math.ceil(total / pageSize), p + 1))} disabled={page >= Math.ceil(total / pageSize)}>›</button>
        <span style={{ color: 'var(--text-muted)', fontSize: 12, margin: '0 4px' }}>每页</span>
        {PAGE_SIZE_OPTIONS.map(s => (
          <button key={s} className={`page-btn ${s === pageSize ? 'active' : ''}`} onClick={() => handlePageSize(s)}>{s}</button>
        ))}
      </div>

      {showModal && (
        <div className="modal-overlay" onClick={() => setShowModal(false)}>
          <div className="modal" onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">新建应用</h2>
            <form onSubmit={handleCreate}>
              <div className="form-group">
                <label className="form-label">应用名称 *</label>
                <input value={form.app_name} onChange={e => setForm({ ...form, app_name: e.target.value })} placeholder="例如：我的授权软件" required />
              </div>
              <div className="form-group">
                <label className="form-label">描述（可选）</label>
                <textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} placeholder="应用描述" rows={3} style={{ resize: 'vertical' }} />
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => setShowModal(false)}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting}>
                  {submitting ? <span className="spinner" /> : '创建'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}

