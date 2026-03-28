import { useEffect, useState } from 'react';
import { cardsApi, appsApi } from '../../lib/api';
import { Plus, Ban, Trash2, RefreshCw, Copy, CheckCircle, Download, Clock, BarChart2 } from 'lucide-react';
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

interface CardGroupStat {
  duration_days: number;
  max_devices: number;
  total: number;
  unused: number;
  active: number;
  expired: number;
  disabled: number;
}

export default function Cards() {
  const [cards, setCards] = useState<Card[]>([]);
  const [apps, setApps] = useState<App[]>([]);
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [total, setTotal] = useState(0);
  const PAGE_SIZE_OPTIONS = [5, 10, 15, 20];
  const [showModal, setShowModal] = useState(false);
  const [showExtendModal, setShowExtendModal] = useState(false);
  const [showStatsModal, setShowStatsModal] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [extendDays, setExtendDays] = useState(30);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [stats, setStats] = useState<CardGroupStat[]>([]);
  const [statsLoading, setStatsLoading] = useState(false);

  const [form, setForm] = useState({
    app_id: '', count: 10, duration_days: 30, max_devices: 1, note: '',
    prefix: 'KAMI', segment_count: 4, segment_len: 4,
  });

  // 搜索过滤状态
  const [searchCode, setSearchCode] = useState('');
  const [filterAppId, setFilterAppId] = useState('');
  const [filterExpireDate, setFilterExpireDate] = useState('');

  const load = (p = page, ps = pageSize) => {
    setLoading(true);
    setCards([]);
    cardsApi.list({ page: p, page_size: ps }).then(res => {
      if (res.data.success) {
        let filtered = res.data.data;
        if (searchCode) filtered = filtered.filter((c: Card) => c.code.toLowerCase().includes(searchCode.toLowerCase()));
        if (filterAppId) filtered = filtered.filter((c: Card) => c.app_id === filterAppId);
        if (filterExpireDate) {
          const fd = new Date(filterExpireDate);
          filtered = filtered.filter((c: Card) => c.expires_at && new Date(c.expires_at).toDateString() === fd.toDateString());
        }
        setCards(filtered);
        setTotal(res.data.total);
      }
    }).finally(() => setLoading(false));
  };

  const handlePageSize = (ps: number) => { setPage(1); setPageSize(ps); };
  const getAppName = (appId: string) => apps.find(a => a.id === appId)?.app_name || '—';

  useEffect(() => {
    appsApi.list().then(res => { if (res.data.success) setApps(res.data.data); });
  }, []);

  useEffect(() => {
    load(page, pageSize);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, pageSize, searchCode, filterAppId, filterExpireDate]);

  const handleGenerate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.app_id) { toast.error('请选择应用'); return; }
    setSubmitting(true);
    try {
      const res = await cardsApi.generate({
        app_id: form.app_id, count: form.count,
        duration_days: form.duration_days, max_devices: form.max_devices,
        note: form.note || undefined,
        prefix: form.prefix || undefined,
        segment_count: form.segment_count,
        segment_len: form.segment_len,
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
      const res = await cardsApi.exportCsv({ app_id: filterAppId || undefined });
      const url = URL.createObjectURL(new Blob([res.data], { type: 'text/csv;charset=utf-8;' }));
      const a = document.createElement('a');
      a.href = url; a.download = `cards_${new Date().toISOString().slice(0, 10)}.csv`;
      a.click(); URL.revokeObjectURL(url);
      toast.success('导出成功');
    } catch { toast.error('导出失败'); }
    finally { setExporting(false); }
  };

  const handleDisable = async (id: string) => {
    try {
      const res = await cardsApi.disable(id);
      if (res.data.success) { toast.success('已禁用'); load(); } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  const handleEnable = async (id: string) => {
    try {
      const res = await cardsApi.enable(id);
      if (res.data.success) { toast.success('已启用'); load(); } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
  };

  const confirm = useConfirm();

  const handleDelete = async (id: string) => {
    const ok = await confirm({ title: '删除卡密', message: '确认删除？仅可删除未使用的卡密，此操作不可撤销。', confirmText: '删除', danger: true });
    if (!ok) return;
    try {
      const res = await cardsApi.delete(id);
      if (res.data.success) { toast.success('删除成功'); load(); } else toast.error(res.data.message);
    } catch { toast.error('删除失败'); }
  };

  const handleBatchExtend = async () => {
    if (selectedIds.length === 0) { toast.error('请先勾选卡密'); return; }
    setSubmitting(true);
    try {
      const res = await cardsApi.batchExtend(selectedIds, extendDays);
      if (res.data.success) {
        toast.success(res.data.message);
        setShowExtendModal(false);
        setSelectedIds([]);
        load();
      } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
    finally { setSubmitting(false); }
  };

  const handleShowStats = async () => {
    setShowStatsModal(true);
    setStatsLoading(true);
    try {
      const res = await cardsApi.stats();
      if (res.data.success) setStats(res.data.data);
    } catch { toast.error('获取统计失败'); }
    finally { setStatsLoading(false); }
  };

  const toggleSelect = (id: string) =>
    setSelectedIds(prev => prev.includes(id) ? prev.filter(i => i !== id) : [...prev, id]);

  const toggleSelectAll = () =>
    setSelectedIds(selectedIds.length === cards.length ? [] : cards.map(c => c.id));

  const copyCode = (code: string) => { navigator.clipboard.writeText(code); toast.success('已复制'); };

  const totalPages = Math.ceil(total / pageSize);
  const statusLabel: Record<string, string> = { unused: '未使用', active: '使用中', expired: '已过期', disabled: '已禁用' };

  // 卡密格式预览
  const formatPreview = (() => {
    const seg = 'X'.repeat(form.segment_len);
    const segs = Array(form.segment_count).fill(seg).join('-');
    return `${form.prefix || 'KAMI'}-${segs}`;
  })();

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">卡密管理</h1>
          <p className="page-subtitle">
            {loading ? <span className="skeleton" style={{ display: 'inline-block', width: 80, height: 13, borderRadius: 4, verticalAlign: 'middle' }} /> : `共 ${total} 张卡密`}
          </p>
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button className="btn btn-ghost" onClick={() => load()}><RefreshCw size={14} /> 刷新</button>
          <button className="btn btn-ghost" onClick={handleShowStats}><BarChart2 size={14} /> 分组统计</button>
          <button className="btn btn-ghost" onClick={handleExport} disabled={exporting}>
            {exporting ? <span className="spinner" /> : <Download size={14} />} 导出 CSV
          </button>
          {selectedIds.length > 0 && (
            <button className="btn btn-ghost" style={{ color: 'var(--accent)', borderColor: 'rgba(124,106,247,0.4)' }}
              onClick={() => setShowExtendModal(true)}>
              <Clock size={14} /> 批量延期 ({selectedIds.length})
            </button>
          )}
          <button className="btn btn-primary" onClick={() => setShowModal(true)}><Plus size={15} /> 生成卡密</button>
        </div>
      </div>

      {/* 搜索过滤 */}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))', gap: 12, marginBottom: 20 }}>
        <div className="form-group" style={{ margin: 0 }}>
          <label className="form-label" style={{ fontSize: 12 }}>搜索卡密</label>
          <input type="text" placeholder="输入卡密代码..." value={searchCode} onChange={e => setSearchCode(e.target.value)} style={{ fontSize: 13 }} />
        </div>
        <div className="form-group" style={{ margin: 0 }}>
          <label className="form-label" style={{ fontSize: 12 }}>按应用过滤</label>
          <select value={filterAppId} onChange={e => setFilterAppId(e.target.value)} style={{ fontSize: 13 }}>
            <option value="">全部应用</option>
            {apps.map(a => <option key={a.id} value={a.id}>{a.app_name}</option>)}
          </select>
        </div>
        <div className="form-group" style={{ margin: 0 }}>
          <label className="form-label" style={{ fontSize: 12 }}>按到期日期过滤</label>
          <input type="date" value={filterExpireDate} onChange={e => setFilterExpireDate(e.target.value)} style={{ fontSize: 13 }} />
        </div>
        {(searchCode || filterAppId || filterExpireDate) && (
          <div style={{ display: 'flex', alignItems: 'flex-end' }}>
            <button className="btn btn-ghost" style={{ fontSize: 12 }}
              onClick={() => { setSearchCode(''); setFilterAppId(''); setFilterExpireDate(''); }}>
              清除过滤
            </button>
          </div>
        )}
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr>
            <th style={{ width: 36 }}>
              <input type="checkbox" checked={selectedIds.length === cards.length && cards.length > 0}
                onChange={toggleSelectAll} />
            </th>
            <th>卡密</th><th>应用</th><th>有效期</th><th>设备上限</th><th>状态</th><th>过期时间</th><th>备注</th><th>操作</th>
          </tr></thead>
          <tbody>
            {loading ? (
              Array.from({ length: pageSize }).map((_, i) => (
                <tr key={i} className="skeleton-row">
                  <td></td>
                  <td><span className="skeleton" style={{ width: '72%' }} /></td>
                  <td><span className="skeleton" style={{ width: '55%' }} /></td>
                  <td><span className="skeleton" style={{ width: '40%' }} /></td>
                  <td><span className="skeleton" style={{ width: '40%' }} /></td>
                  <td><span className="skeleton" style={{ width: '52%' }} /></td>
                  <td><span className="skeleton" style={{ width: '60%' }} /></td>
                  <td><span className="skeleton" style={{ width: '45%' }} /></td>
                  <td><span className="skeleton" style={{ width: '64px', height: '28px' }} /></td>
                </tr>
              ))
            ) : cards.length === 0 ? (
              <tr><td colSpan={9}><div className="empty-state"><div className="empty-state-icon">🔑</div><div className="empty-state-text">暂无卡密，点击「生成卡密」</div></div></td></tr>
            ) : cards.map((card, idx) => (
              <tr key={card.id} className="data-enter" style={{ animationDelay: `${idx * 30}ms` }}>
                <td><input type="checkbox" checked={selectedIds.includes(card.id)} onChange={() => toggleSelect(card.id)} /></td>
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

      {/* ── 生成卡密弹窗 ── */}
      {showModal && (
        <div className="modal-overlay" onClick={() => setShowModal(false)}>
          <div className="modal" onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">批量生成卡密</h2>
            <form onSubmit={handleGenerate}>
              <div className="form-group">
                <label className="form-label">选择应用 *</label>
                <select value={form.app_id} onChange={e => setForm({ ...form, app_id: e.target.value })} required>
                  <option value="">请选择应用</option>
                  {apps.map(a => <option key={a.id} value={a.id}>{a.app_name}</option>)}
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

              {/* 格式自定义 */}
              <div style={{ borderTop: '1px solid var(--border)', paddingTop: 12, marginTop: 4 }}>
                <div style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 10 }}>卡密格式自定义</div>
                <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 10 }}>
                  <div className="form-group" style={{ margin: 0 }}>
                    <label className="form-label" style={{ fontSize: 11 }}>前缀</label>
                    <input value={form.prefix} maxLength={16}
                      onChange={e => setForm({ ...form, prefix: e.target.value.toUpperCase() })}
                      placeholder="KAMI" style={{ fontSize: 13 }} />
                  </div>
                  <div className="form-group" style={{ margin: 0 }}>
                    <label className="form-label" style={{ fontSize: 11 }}>段数 (1-8)</label>
                    <input type="number" min={1} max={8} value={form.segment_count}
                      onChange={e => setForm({ ...form, segment_count: +e.target.value })} style={{ fontSize: 13 }} />
                  </div>
                  <div className="form-group" style={{ margin: 0 }}>
                    <label className="form-label" style={{ fontSize: 11 }}>每段长度 (2-8)</label>
                    <input type="number" min={2} max={8} value={form.segment_len}
                      onChange={e => setForm({ ...form, segment_len: +e.target.value })} style={{ fontSize: 13 }} />
                  </div>
                </div>
                <div style={{ marginTop: 8, fontSize: 12, color: 'var(--accent)', fontFamily: 'monospace', letterSpacing: 1 }}>
                  预览：{formatPreview}
                </div>
              </div>

              <div className="form-group" style={{ marginTop: 12 }}>
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

      {/* ── 批量延期弹窗 ── */}
      {showExtendModal && (
        <div className="modal-overlay" onClick={() => setShowExtendModal(false)}>
          <div className="modal" style={{ maxWidth: 400 }} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">批量调整有效期</h2>
            <p style={{ fontSize: 13, color: 'var(--text-muted)', marginBottom: 16 }}>
              已选 <strong style={{ color: 'var(--accent)' }}>{selectedIds.length}</strong> 张卡密。
              正数延期，负数缩短（已激活卡密缩短后不早于当前时间）。
            </p>
            <div className="form-group">
              <label className="form-label">调整天数</label>
              <input type="number" value={extendDays}
                onChange={e => setExtendDays(+e.target.value)}
                placeholder="正数延期，负数缩短" />
            </div>
            <div className="modal-actions">
              <button className="btn btn-ghost" onClick={() => setShowExtendModal(false)}>取消</button>
              <button className="btn btn-primary" disabled={submitting || extendDays === 0} onClick={handleBatchExtend}>
                {submitting ? <span className="spinner" /> : `确认${extendDays > 0 ? '延期' : '缩短'} ${Math.abs(extendDays)} 天`}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ── 分组统计弹窗 ── */}
      {showStatsModal && (
        <div className="modal-overlay" onClick={() => setShowStatsModal(false)}>
          <div className="modal" style={{ maxWidth: 640 }} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">卡密分组统计</h2>
            {statsLoading ? (
              <div style={{ padding: '32px 0', textAlign: 'center' }}>
                <span className="spinner" style={{ width: 24, height: 24 }} />
              </div>
            ) : stats.length === 0 ? (
              <div className="empty-state"><div className="empty-state-text">暂无数据</div></div>
            ) : (
              <div className="table-wrap" style={{ marginTop: 0 }}>
                <table>
                  <thead><tr>
                    <th>有效期</th><th>设备上限</th><th>总数</th>
                    <th>未使用</th><th>使用中</th><th>已过期</th><th>已禁用</th>
                  </tr></thead>
                  <tbody>
                    {stats.map((s, i) => (
                      <tr key={i}>
                        <td>{s.duration_days} 天</td>
                        <td>{s.max_devices} 台</td>
                        <td><strong>{s.total}</strong></td>
                        <td style={{ color: '#34d399' }}>{s.unused}</td>
                        <td style={{ color: '#60a5fa' }}>{s.active}</td>
                        <td style={{ color: 'var(--text-muted)' }}>{s.expired}</td>
                        <td style={{ color: '#f87171' }}>{s.disabled}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            <div className="modal-actions">
              <button className="btn btn-ghost" onClick={() => setShowStatsModal(false)}>关闭</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
 