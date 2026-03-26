import { useEffect, useState } from 'react';
import { cardsApi, appsApi } from '../../lib/api';
import { Plus, Ban, Trash2, RefreshCw, Copy, CheckCircle, Download } from 'lucide-react';
import toast from 'react-hot-toast';
import { useConfirm } from '../../stores/confirm';

interface Card {
  id: string;
  app_id: string;
  code: string;
  duration_days: number;
  max_devices: number;
  status: string;
  note: string | null;
  created_at: string;
  activated_at: string | null;
  expires_at: string | null;
}

interface App { id: string; app_name: string; }

export default function Cards() {
  const [cards, setCards] = useState<Card[]>([]);
  const [apps, setApps] = useState<App[]>([]);
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [total, setTotal] = useState(0);
  const PAGE_SIZE_OPTIONS = [5, 10, 15, 20];
  const [showModal, setShowModal] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [form, setForm] = useState({ app_id: '', count: 10, duration_days: 30, max_devices: 1, note: '' });
  
  // 搜索过滤状态
  const [searchCode, setSearchCode] = useState('');
  const [filterAppId, setFilterAppId] = useState('');
  const [filterExpireDate, setFilterExpireDate] = useState('');

  const load = (p = page, ps = pageSize) => {
    setLoading(true);
    cardsApi.list({ page: p, page_size: ps }).then(res => {
      if (res.data.success) { 
        let filtered = res.data.data;
        
        // 按卡密代码搜索
        if (searchCode) {
          filtered = filtered.filter((c: Card) => c.code.toLowerCase().includes(searchCode.toLowerCase()));
        }
        
        // 按应用过滤
        if (filterAppId) {
          filtered = filtered.filter((c: Card) => c.app_id === filterAppId);
        }
        
        // 按到期时间过滤
        if (filterExpireDate) {
          const filterDate = new Date(filterExpireDate);
          filtered = filtered.filter((c: Card) => {
            if (!c.expires_at) return false;
            const expiresDate = new Date(c.expires_at);
            return expiresDate.toDateString() === filterDate.toDateString();
          });
        }
        
        setCards(filtered); 
        setTotal(res.data.total); 
      }
    }).finally(() => setLoading(false));
  };

  const handlePageSize = (ps: number) => { setPageSize(ps); setPage(1); };
  
  const getAppName = (appId: string) => {
    return apps.find(a => a.id === appId)?.app_name || '—';
  };

  useEffect(() => {
    load(1, pageSize);
    appsApi.list().then(res => { if (res.data.success) setApps(res.data.data); });
  }, []);

  useEffect(() => { load(page, pageSize); }, [page, pageSize]);
  
  // 搜索/过滤时重置到第一页
  useEffect(() => { setPage(1); }, [searchCode, filterAppId, filterExpireDate]);

  const handleGenerate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.app_id) { toast.error('请选择应用'); return; }
    setSubmitting(true);
    try {
      const res = await cardsApi.generate({
        app_id: form.app_id, count: form.count,
        duration_days: form.duration_days, max_devices: form.max_devices,
        note: form.note || undefined,
      });
      if (res.data.success) {
        toast.success(res.data.message);
        setShowModal(false);
        setPage(1); load(1, pageSize);
      } else toast.error(res.data.message);
    } catch { toast.error('生成失败'); }
    finally { setSubmitting(false); }
  };

  const handleExport = async () => {
    setExporting(true);
    try {
      const res = await cardsApi.exportCsv({
        app_id: filterAppId || undefined,
      });
      const url = URL.createObjectURL(new Blob([res.data], { type: 'text/csv;charset=utf-8;' }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `cards_${new Date().toISOString().slice(0, 10)}.csv`;
      a.click();
      URL.revokeObjectURL(url);
      toast.success('导出成功');
    } catch { toast.error('导出失败'); }
    finally { setExporting(false); }
  };

  const handleDisable = async (id: string) => {
    try {
      const res = await cardsApi.disable(id);
      if (res.data.success) { toast.success('已禁用'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  const handleEnable = async (id: string) => {
    try {
      const res = await cardsApi.enable(id);
      if (res.data.success) { toast.success('已启用'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  const confirm = useConfirm();

  const handleDelete = async (id: string) => {
    const ok = await confirm({ title: '删除卡密', message: '确认删除？仅可删除未使用的卡密，此操作不可撤销。', confirmText: '删除', danger: true });
    if (!ok) return;
    try {
      const res = await cardsApi.delete(id);
      if (res.data.success) { toast.success('删除成功'); load(); }
      else toast.error(res.data.message);
    } catch { toast.error('删除失败'); }
  };

  const copyCode = (code: string) => {
    navigator.clipboard.writeText(code);
    toast.success('已复制');
  };

  const totalPages = Math.ceil(total / pageSize);

  const statusLabel: Record<string, string> = { unused: '未使用', active: '使用中', expired: '已过期', disabled: '已禁用' };

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">卡密管理</h1>
          <p className="page-subtitle">共 {total} 张卡密</p>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => load()}><RefreshCw size={14} /> 刷新</button>
          <button className="btn btn-ghost" onClick={handleExport} disabled={exporting}>
            {exporting ? <span className="spinner" /> : <Download size={14} />} 导出 CSV
          </button>
          <button className="btn btn-primary" onClick={() => setShowModal(true)}><Plus size={15} /> 生成卡密</button>
        </div>
      </div>

      {/* 搜索过滤区域 */}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))', gap: 12, marginBottom: 20 }}>
        <div className="form-group" style={{ margin: 0 }}>
          <label className="form-label" style={{ fontSize: 12 }}>搜索卡密</label>
          <input 
            type="text" 
            placeholder="输入卡密代码..." 
            value={searchCode}
            onChange={e => setSearchCode(e.target.value)}
            style={{ fontSize: 13 }}
          />
        </div>
        <div className="form-group" style={{ margin: 0 }}>
          <label className="form-label" style={{ fontSize: 12 }}>按应用过滤</label>
          <select 
            value={filterAppId}
            onChange={e => setFilterAppId(e.target.value)}
            style={{ fontSize: 13 }}
          >
            <option value="">全部应用</option>
            {apps.map(a => (
              <option key={a.id} value={a.id}>{a.app_name}</option>
            ))}
          </select>
        </div>
        <div className="form-group" style={{ margin: 0 }}>
          <label className="form-label" style={{ fontSize: 12 }}>按到期日期过滤</label>
          <input 
            type="date" 
            value={filterExpireDate}
            onChange={e => setFilterExpireDate(e.target.value)}
            style={{ fontSize: 13 }}
          />
        </div>
        {(searchCode || filterAppId || filterExpireDate) && (
          <div style={{ display: 'flex', alignItems: 'flex-end' }}>
            <button 
              className="btn btn-ghost" 
              onClick={() => { setSearchCode(''); setFilterAppId(''); setFilterExpireDate(''); }}
              style={{ fontSize: 12 }}
            >
              清除过滤
            </button>
          </div>
        )}
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr>
            <th>卡密</th><th>应用</th><th>有效期</th><th>设备上限</th><th>状态</th><th>过期时间</th><th>备注</th><th>操作</th>
          </tr></thead>
          <tbody>
            {loading ? (
              <tr><td colSpan={8} style={{ textAlign: 'center', padding: 40 }}><span className="spinner" /></td></tr>
            ) : cards.length === 0 ? (
              <tr><td colSpan={8}><div className="empty-state"><div className="empty-state-icon">🔑</div><div className="empty-state-text">暂无卡密，点击「生成卡密」</div></div></td></tr>
            ) : cards.map(card => (
              <tr key={card.id}>
                <td>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    <span className="mono" style={{ fontSize: 12, color: 'var(--accent)', letterSpacing: '1px' }}>{card.code}</span>
                    <button style={{ background: 'none', color: 'var(--text-muted)', padding: 2, borderRadius: 4 }} onClick={() => copyCode(card.code)}><Copy size={12} /></button>
                  </div>
                </td>
                <td><span style={{ fontSize: 12, color: 'var(--text-muted)' }}>{getAppName(card.app_id)}</span></td>
                <td>{card.duration_days} 天</td>
                <td>{card.max_devices} 台</td>
                <td><span className={`badge badge-${card.status}`}>{statusLabel[card.status]}</span></td>
                <td>{card.expires_at ? new Date(card.expires_at).toLocaleDateString('zh-CN') : '—'}</td>
                <td><span style={{ color: 'var(--text-muted)' }}>{card.note || '—'}</span></td>
                <td>
                  <div style={{ display: 'flex', gap: 6 }}>
                    {card.status !== 'disabled' && (
                      <button className="btn btn-sm btn-danger" onClick={() => handleDisable(card.id)}><Ban size={12} /></button>
                    )}
                    {card.status === 'disabled' && (
                      <button className="btn btn-sm btn-ghost" style={{ color: 'var(--success)', borderColor: 'rgba(52,211,153,0.3)' }} onClick={() => handleEnable(card.id)}><CheckCircle size={12} /></button>
                    )}
                    {card.status === 'unused' && (
                      <button className="btn btn-sm btn-ghost" onClick={() => handleDelete(card.id)}><Trash2 size={12} /></button>
                    )}
                  </div>
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
        <button className="page-btn" onClick={() => setPage(p => Math.min(totalPages, p + 1))} disabled={page >= totalPages}>›</button>
        <span style={{ color: 'var(--text-muted)', fontSize: 12, margin: '0 4px' }}>每页</span>
        {PAGE_SIZE_OPTIONS.map(s => (
          <button key={s} className={`page-btn ${s === pageSize ? 'active' : ''}`} onClick={() => handlePageSize(s)}>{s}</button>
        ))}
      </div>

      {showModal && (
        <div className="modal-overlay" onClick={() => setShowModal(false)}>
          <div className="modal" onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">批量生成卡密</h2>
            <form onSubmit={handleGenerate}>
              <div className="form-group">
                <label className="form-label">选择应用 *</label>
                <select value={form.app_id} onChange={e => setForm({ ...form, app_id: e.target.value })} required>
                  <option value="">请选择应用</option>
                  {apps.filter(a => a).map(a => (
                    <option key={a.id} value={a.id}>{a.app_name}</option>
                  ))}
                </select>
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
                <div className="form-group">
                  <label className="form-label">生成数量</label>
                  <input type="number" min={1} max={1000} value={form.count} onChange={e => setForm({ ...form, count: +e.target.value })} required />
                </div>
                <div className="form-group">
                  <label className="form-label">有效天数</label>
                  <input type="number" min={1} value={form.duration_days} onChange={e => setForm({ ...form, duration_days: +e.target.value })} required />
                </div>
              </div>
              <div className="form-group">
                <label className="form-label">最大设备数（每张）</label>
                <input type="number" min={1} max={100} value={form.max_devices} onChange={e => setForm({ ...form, max_devices: +e.target.value })} required />
              </div>
              <div className="form-group">
                <label className="form-label">备注（可选）</label>
                <input value={form.note} onChange={e => setForm({ ...form, note: e.target.value })} placeholder="如：2024年批次" />
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => setShowModal(false)}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting}>
                  {submitting ? <span className="spinner" /> : '生成'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}

