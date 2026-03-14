import { useEffect, useState } from 'react';
import { activationsApi } from '../../lib/api';
import { Unlink, RefreshCw } from 'lucide-react';
import toast from 'react-hot-toast';

interface Activation {
  id: string;
  card_id: string;
  app_id: string;
  device_id: string;
  device_name: string | null;
  ip_address: string | null;
  activated_at: string;
  last_verified_at: string;
}

export default function Activations() {
  const [list, setList] = useState<Activation[]>([]);
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [total, setTotal] = useState(0);
  const PAGE_SIZE = 20;

  const load = (p = page) => {
    setLoading(true);
    activationsApi.list({ page: p, page_size: PAGE_SIZE }).then(res => {
      if (res.data.success) { setList(res.data.data); setTotal(res.data.total); }
    }).finally(() => setLoading(false));
  };

  useEffect(() => { load(); }, []);
  useEffect(() => { load(page); }, [page]);

  const handleUnbind = async (id: string) => {
    if (!confirm('确认解绑此设备？')) return;
    try {
      const res = await activationsApi.unbind(id);
      if (res.data.success) { toast.success('设备已解绑'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  const totalPages = Math.ceil(total / PAGE_SIZE);

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">激活记录</h1>
          <p className="page-subtitle">共 {total} 条激活记录</p>
        </div>
        <button className="btn btn-ghost" onClick={() => load()}><RefreshCw size={14} /> 刷新</button>
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr>
            <th>设备 ID</th><th>设备名称</th><th>IP 地址</th><th>激活时间</th><th>最后验证</th><th>操作</th>
          </tr></thead>
          <tbody>
            {loading ? (
              <tr><td colSpan={6} style={{ textAlign: 'center', padding: 40 }}><span className="spinner" /></td></tr>
            ) : list.length === 0 ? (
              <tr><td colSpan={6}><div className="empty-state"><div className="empty-state-icon">📡</div><div className="empty-state-text">暂无激活记录</div></div></td></tr>
            ) : list.map(a => (
              <tr key={a.id}>
                <td><span className="mono" style={{ fontSize: 11, color: 'var(--text-dim)' }}>{a.device_id.slice(0, 20)}…</span></td>
                <td>{a.device_name || '—'}</td>
                <td><span className="mono" style={{ fontSize: 12 }}>{a.ip_address || '—'}</span></td>
                <td>{new Date(a.activated_at).toLocaleString('zh-CN')}</td>
                <td>{new Date(a.last_verified_at).toLocaleString('zh-CN')}</td>
                <td>
                  <button className="btn btn-sm btn-danger" onClick={() => handleUnbind(a.id)}>
                    <Unlink size={12} /> 解绑
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {totalPages > 1 && (
        <div className="pagination">
          <button className="page-btn" onClick={() => setPage(p => Math.max(1, p - 1))} disabled={page === 1}>‹</button>
          {Array.from({ length: totalPages }, (_, i) => i + 1)
            .slice(Math.max(0, page - 3), Math.min(totalPages, page + 2))
            .map(p => (
              <button key={p} className={`page-btn ${p === page ? 'active' : ''}`} onClick={() => setPage(p)}>{p}</button>
            ))}
          <button className="page-btn" onClick={() => setPage(p => Math.min(totalPages, p + 1))} disabled={page === totalPages}>›</button>
        </div>
      )}
    </div>
  );
}

