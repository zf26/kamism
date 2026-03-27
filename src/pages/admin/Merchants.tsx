import { useEffect, useState } from 'react';
import { adminApi } from '../../lib/api';
import { CheckCircle, XCircle, RefreshCw, Search, Crown, Gift, Clock } from 'lucide-react';
import toast from 'react-hot-toast';

interface Merchant {
  id: string;
  username: string;
  email: string;
  api_key: string;
  status: string;
  plan: string;
  plan_expires_at: string | null;
  email_verified: boolean;
  created_at: string;
}

interface PlanConfig {
  plan: string;
  label: string;
  max_apps: number;
  max_cards: number;
  max_devices: number;
  max_gen_once: number;
}

// 升级弹窗状态
interface UpgradeModal {
  merchantId: string;
  username: string;
}

const PAGE_SIZE_OPTIONS = [5, 10, 15, 20];

function displayVal(v: number) {
  return v === -1 ? '无限' : String(v);
}

function formatExpiry(expiresAt: string | null, plan: string) {
  if (plan !== 'pro') return null;
  if (!expiresAt) return <span style={{ color: '#a78bfa', fontSize: 11 }}>永久</span>;
  const d = new Date(expiresAt);
  const now = new Date();
  const days = Math.ceil((d.getTime() - now.getTime()) / 86400000);
  const color = days <= 3 ? '#ef4444' : days <= 7 ? '#f59e0b' : '#a78bfa';
  return (
    <span style={{ color, fontSize: 11, display: 'flex', alignItems: 'center', gap: 3 }}>
      <Clock size={10} />
      {days <= 0 ? '已到期' : `${days}天后到期`}
    </span>
  );
}

export default function Merchants() {
  const [merchants, setMerchants] = useState<Merchant[]>([]);
  const [planConfigs, setPlanConfigs] = useState<Record<string, PlanConfig>>({});
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [total, setTotal] = useState(0);
  const [keyword, setKeyword] = useState('');
  const [search, setSearch] = useState('');
  const [planFilter, setPlanFilter] = useState<string>('');
  const [planLoading, setPlanLoading] = useState<string | null>(null);
  const [upgradeModal, setUpgradeModal] = useState<UpgradeModal | null>(null);
  const [expiresDays, setExpiresDays] = useState<string>('30');

  const loadPlanConfigs = () => {
    adminApi.getPlanConfigs().then(res => {
      if (res.data.success) {
        const map: Record<string, PlanConfig> = {};
        (res.data.data as PlanConfig[]).forEach(c => { map[c.plan] = c; });
        setPlanConfigs(map);
      }
    });
  };

  const load = (p = page, ps = pageSize, kw = search, pf = planFilter) => {
    setLoading(true);
    setMerchants([]);
    adminApi.getMerchants({ page: p, page_size: ps, keyword: kw || undefined, plan: pf || undefined })
      .then(res => {
        if (res.data.success) {
          setMerchants(res.data.data);
          setTotal(res.data.total);
        }
      }).finally(() => setLoading(false));
  };

  useEffect(() => {
    loadPlanConfigs();
    load(page, pageSize, search, planFilter);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, pageSize, search, planFilter]);

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    setPage(1);
    setSearch(keyword);
    // page/search 变化由 useEffect 统一驱动，无需手动调用 load()
  };

  const handlePlanFilter = (pf: string) => {
    setPlanFilter(pf);
    setPage(1);
  };

  const handlePageSize = (ps: number) => {
    setPage(1);
    setPageSize(ps);
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

  const togglePlan = async (id: string, current: string) => {
    // 降为免费版直接操作，升级专业版弹窗输入天数
    if (current !== 'pro') {
      const m = merchants.find(m => m.id === id);
      setUpgradeModal({ merchantId: id, username: m?.username ?? '' });
      setExpiresDays('30');
      return;
    }
    // 降为免费
    setPlanLoading(id);
    try {
      const res = await adminApi.updateMerchantPlan(id, 'free');
      if (res.data.success) {
        toast.success('已降级为免费版');
        load();
      } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
    finally { setPlanLoading(null); }
  };

  const confirmUpgrade = async () => {
    if (!upgradeModal) return;
    const days = expiresDays === '' ? undefined : parseInt(expiresDays);
    if (days !== undefined && (isNaN(days) || days < 1)) {
      toast.error('请输入有效天数（留空为永久）');
      return;
    }
    setPlanLoading(upgradeModal.merchantId);
    setUpgradeModal(null);
    try {
      const res = await adminApi.updateMerchantPlan(upgradeModal.merchantId, 'pro', days);
      if (res.data.success) {
        toast.success(res.data.message ?? '已升级为专业版');
        load();
      } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
    finally { setPlanLoading(null); }
  };

  const totalPages = Math.ceil(total / pageSize);

  return (
    <div className="fade-in">
      {/* 升级专业版弹窗 */}
      {upgradeModal && (
        <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.6)', zIndex: 1000, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
          <div style={{ background: 'var(--bg-card)', border: '1px solid var(--border)', borderRadius: 12, padding: 28, width: 360, display: 'flex', flexDirection: 'column', gap: 16 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <Crown size={18} color="#a78bfa" />
              <h3 style={{ fontWeight: 700, fontSize: 16 }}>升级专业版</h3>
            </div>
            <p style={{ color: 'var(--text-muted)', fontSize: 13 }}>为商户 <strong style={{ color: 'var(--text)' }}>{upgradeModal.username}</strong> 设置专业版有效期</p>
            <div>
              <label style={{ fontSize: 12, color: 'var(--text-muted)', display: 'block', marginBottom: 6 }}>
                有效天数（留空为永久）
              </label>
              <input
                type="number"
                min={1}
                value={expiresDays}
                onChange={e => setExpiresDays(e.target.value)}
                placeholder="例如：30"
                style={{ width: '100%' }}
                autoFocus
              />
            </div>
            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
              <button className="btn btn-ghost" onClick={() => setUpgradeModal(null)}>取消</button>
              <button className="btn btn-primary" onClick={confirmUpgrade}>
                <Crown size={13} /> 确认升级
              </button>
            </div>
          </div>
        </div>
      )}
      <div className="page-header">
        <div>
          <h1 className="page-title">商户管理</h1>
          <p className="page-subtitle">
            {loading ? <span className="skeleton" style={{ display: 'inline-block', width: 80, height: 13, borderRadius: 4, verticalAlign: 'middle' }} /> : `共 ${total} 个商户`}
          </p>
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

      {/* 套餐筛选 Tab */}
      <div style={{ display: 'flex', gap: 6, marginBottom: 16 }}>
        {[{ value: '', label: '全部' }, { value: 'free', label: '免费版' }, { value: 'pro', label: '专业版' }].map(tab => (
          <button
            key={tab.value}
            onClick={() => handlePlanFilter(tab.value)}
            style={{
              padding: '5px 16px', borderRadius: 999, fontSize: 13, fontWeight: 600,
              border: '1px solid',
              borderColor: planFilter === tab.value ? (tab.value === 'pro' ? 'rgba(124,58,237,0.6)' : 'var(--primary)') : 'var(--border)',
              background: planFilter === tab.value ? (tab.value === 'pro' ? 'rgba(124,58,237,0.15)' : 'rgba(124,106,247,0.12)') : 'transparent',
              color: planFilter === tab.value ? (tab.value === 'pro' ? '#a78bfa' : 'var(--primary-light)') : 'var(--text-muted)',
              cursor: 'pointer', transition: 'all 0.15s',
            }}
          >
            {tab.value === 'pro' && <Crown size={11} style={{ marginRight: 4, verticalAlign: 'middle' }} />}
            {tab.value === 'free' && <Gift size={11} style={{ marginRight: 4, verticalAlign: 'middle' }} />}
            {tab.label}
          </button>
        ))}
      </div>

      {/* 套餐说明 */}
      <div style={{ display: 'flex', gap: 12, marginBottom: 20, flexWrap: 'wrap' }}>
        {Object.values(planConfigs).map((config) => (
          <div key={config.plan} style={{
            display: 'flex', alignItems: 'center', gap: 10,
            padding: '10px 16px', borderRadius: 8,
            background: config.plan === 'pro' ? 'rgba(124,58,237,0.08)' : 'var(--surface)',
            border: `1px solid ${config.plan === 'pro' ? 'rgba(124,58,237,0.3)' : 'var(--border)'}`,
            fontSize: 13,
          }}>
            {config.plan === 'pro' ? <Crown size={14} color="#a78bfa" /> : <Gift size={14} color="var(--text-muted)" />}
            <span style={{ fontWeight: 600, color: config.plan === 'pro' ? '#a78bfa' : 'var(--text)' }}>
              {config.label}
            </span>
            <span style={{ color: 'var(--text-muted)' }}>
              应用 {displayVal(config.max_apps)} · 卡密 {displayVal(config.max_cards)} · 设备 {displayVal(config.max_devices)}/张
            </span>
          </div>
        ))}
      </div>

      <div className="table-wrap">
        <table>
          <thead><tr>
            <th>用户名</th><th>邮箱</th><th>API Key</th><th>套餐</th><th>到期时间</th><th>状态</th><th>注册时间</th><th>操作</th>
          </tr></thead>
          <tbody>
            {loading ? (
              Array.from({ length: pageSize }).map((_, i) => (
                <tr key={i} className="skeleton-row">
                  <td><span className="skeleton" style={{ width: '60%' }} /></td>
                  <td><span className="skeleton" style={{ width: '75%' }} /></td>
                  <td><span className="skeleton" style={{ width: '80px' }} /></td>
                  <td><span className="skeleton" style={{ width: '64px', height: '22px', borderRadius: 999 }} /></td>
                  <td><span className="skeleton" style={{ width: '70px' }} /></td>
                  <td><span className="skeleton" style={{ width: '48px', height: '22px', borderRadius: 999 }} /></td>
                  <td><span className="skeleton" style={{ width: '55%' }} /></td>
                  <td><span className="skeleton" style={{ width: '120px', height: '28px' }} /></td>
                </tr>
              ))
            ) : merchants.length === 0 ? (
              <tr><td colSpan={7}><div className="empty-state"><div className="empty-state-icon">👤</div><div className="empty-state-text">暂无商户</div></div></td></tr>
            ) : merchants.map((m, idx) => (
              <tr key={m.id} className="data-enter" style={{ animationDelay: `${idx * 25}ms` }}>
                <td><span style={{ color: 'var(--text)', fontWeight: 600 }}>{m.username}</span></td>
                <td>{m.email}</td>
                <td><span className="mono" style={{ fontSize: 11, color: 'var(--text-muted)', letterSpacing: '0.5px' }}>{m.api_key.slice(0, 12)}…</span></td>
                <td>
                  <span style={{
                    display: 'inline-flex', alignItems: 'center', gap: 4,
                    fontSize: 12, fontWeight: 600, padding: '2px 8px', borderRadius: 999,
                    background: m.plan === 'pro' ? 'rgba(124,58,237,0.15)' : 'rgba(255,255,255,0.06)',
                    color: m.plan === 'pro' ? '#a78bfa' : 'var(--text-muted)',
                    border: `1px solid ${m.plan === 'pro' ? 'rgba(124,58,237,0.35)' : 'var(--border)'}`,
                  }}>
                    {m.plan === 'pro' ? <><Crown size={10} /> 专业版</> : <><Gift size={10} /> 免费版</>}
                  </span>
                </td>
                <td>{formatExpiry(m.plan_expires_at, m.plan) ?? <span style={{ color: 'var(--text-muted)', fontSize: 12 }}>—</span>}</td>
                <td><span className={`badge badge-${m.status}`}>{m.status === 'active' ? '正常' : '禁用'}</span></td>
                <td>{new Date(m.created_at).toLocaleDateString('zh-CN')}</td>
                <td>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <button
                      className={`btn btn-sm ${m.plan === 'pro' ? 'btn-ghost' : 'btn-primary'}`}
                      onClick={() => togglePlan(m.id, m.plan)}
                      disabled={planLoading === m.id}
                      style={{ display: 'inline-flex', alignItems: 'center', gap: 4, fontSize: 12 }}
                    >
                      {planLoading === m.id
                        ? <span className="spinner" style={{ width: 10, height: 10 }} />
                        : m.plan === 'pro'
                          ? <><Gift size={11} /> 降为免费</>
                          : <><Crown size={11} /> 升专业版</>
                      }
                    </button>
                    <button
                      className={`btn btn-sm ${m.status === 'active' ? 'btn-danger' : 'btn-ghost'}`}
                      onClick={() => toggleStatus(m.id, m.status)}
                      style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}
                    >
                      {m.status === 'active' ? <><XCircle size={12} /> 禁用</> : <><CheckCircle size={12} /> 启用</>}
                    </button>
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
