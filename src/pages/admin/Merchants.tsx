import { useEffect, useState } from 'react';
import { adminApi } from '../../lib/api';
import { CheckCircle, XCircle, RefreshCw, Search } from 'lucide-react';
import toast from 'react-hot-toast';

interface Merchant {
  id: string;
  username: string;
  email: string;
  api_key: string;
  status: string;
  email_verified: boolean;
  created_at: string;
}

const PAGE_SIZE_OPTIONS = [5, 10, 15, 20];

export default function Merchants() {
  const [merchants, setMerchants] = useState<Merchant[]>([]);
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [total, setTotal] = useState(0);
  const [keyword, setKeyword] = useState('');
  const [search, setSearch] = useState('');

  const load = (p = page, ps = pageSize, kw = search) => {
    setLoading(true);
    adminApi.getMerchants({ page: p, page_size: ps, keyword: kw || undefined })
      .then(res => {
        if (res.data.success) {
          setMerchants(res.data.data);
          setTotal(res.data.total);
        }
      }).finally(() => setLoading(false));
  };

  useEffect(() => { load(page, pageSize, search); }, [page, pageSize]);

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    setPage(1);
    setSearch(keyword);
    load(1, pageSize, keyword);
  };

  const handlePageSize = (ps: number) => {
    setPageSize(ps);
    setPage(1);
  };

  const toggleStatus = async (id: string, current: string) => {
    const next = current === 'active' ? 'disabled' : 'active';
    try {
      const res = await adminApi.updateMerchantStatus(id, next);
      if (res.data.success) {
        toast.success(`已${next === 'active' ? '启用' : '禁用'}`);
        load();
      } else {
        toast.error(res.data.message);
      }
    } catch {
      toast.error('操作失败');
    }
  };

  const totalPages = Math.ceil(total / pageSize);

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">商户管理</h1>
          <p className="page-subtitle">共 {total} 个商户</p>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <form onSubmit={handleSearch} style={{ display: 'flex', gap: 6 }}>
            <div style={{ position: 'relative' }}>
              <Search size={14} style={{ position: 'absolute', left: 10, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
              <input
                value={keyword}
                onChange={e => setKeyword(e.target.value)}
                placeholder="搜索用户名/邮箱"
                style={{ paddingLeft: 32, width: 200 }}
              />
            </div>
            <button type="submit" className="btn btn-ghost"><Search size={14} /></button>
          </form>
          <button className="btn btn-ghost" onClick={() => load()}><RefreshCw size={14} /> 刷新</button>
        </div>
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr>
            <th>用户名</th><th>邮箱</th><th>API Key</th><th>状态</th><th>注册时间</th><th>操作</th>
          </tr></thead>
          <tbody>
            {loading ? (
              <tr><td colSpan={6} style={{ textAlign: 'center', padding: 40, color: 'var(--text-muted)' }}><span className="spinner" /></td></tr>
            ) : merchants.length === 0 ? (
              <tr><td colSpan={6}><div className="empty-state"><div className="empty-state-icon">👤</div><div className="empty-state-text">暂无商户</div></div></td></tr>
            ) : merchants.map(m => (
              <tr key={m.id}>
                <td><span style={{ color: 'var(--text)', fontWeight: 600 }}>{m.username}</span></td>
                <td>{m.email}</td>
                <td><span className="mono" style={{ fontSize: 11, color: 'var(--text-muted)', letterSpacing: '0.5px' }}>{m.api_key.slice(0, 12)}…</span></td>
                <td><span className={`badge badge-${m.status}`}>{m.status === 'active' ? '正常' : '禁用'}</span></td>
                <td>{new Date(m.created_at).toLocaleDateString('zh-CN')}</td>
                <td>
                  <button
                    className={`btn btn-sm ${m.status === 'active' ? 'btn-danger' : 'btn-ghost'}`}
                    onClick={() => toggleStatus(m.id, m.status)}
                    style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}
                  >
                    {m.status === 'active' ? <><XCircle size={12} /> 禁用</> : <><CheckCircle size={12} /> 启用</>}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="pagination">
        <button className="page-btn" onClick={() => setPage(p => Math.max(1, p - 1))} disabled={page === 1}>‹</button>
        {Array.from({ length: totalPages }, (_, i) => i + 1)
          .slice(Math.max(0, page - 3), Math.min(totalPages, page + 2))
          .map(p => (
            <button key={p} className={`page-btn ${p === page ? 'active' : ''}`} onClick={() => setPage(p)}>{p}</button>
          ))}
        <button className="page-btn" onClick={() => setPage(p => Math.min(totalPages, p + 1))} disabled={page === totalPages || totalPages === 0}>›</button>
        <span style={{ color: 'var(--text-muted)', fontSize: 12, margin: '0 4px' }}>每页</span>
        {PAGE_SIZE_OPTIONS.map(s => (
          <button
            key={s}
            className={`page-btn ${s === pageSize ? 'active' : ''}`}
            onClick={() => handlePageSize(s)}
          >{s}</button>
        ))}
      </div>
    </div>
  );
}
