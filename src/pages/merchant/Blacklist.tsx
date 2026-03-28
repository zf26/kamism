import { useEffect, useState } from 'react';
import { blacklistApi } from '../../lib/api';
import { Plus, Trash2, RefreshCw, Shield, Monitor, AlertTriangle, CheckCircle } from 'lucide-react';
import toast from 'react-hot-toast';
import { useConfirm } from '../../stores/confirm';

interface IpEntry { id: string; ip: string; reason: string | null; created_at: string; }
interface DeviceEntry { id: string; device_hint: string | null; reason: string | null; created_at: string; }
interface AlertEntry {
  id: string;
  alert_type: string;
  card_id: string | null;
  device_hint: string | null;
  ip_address: string | null;
  detail: string | null;
  is_read: boolean;
  created_at: string;
}

const ALERT_LABELS: Record<string, string> = {
  ip_abuse: 'IP 频繁激活',
  device_multi_card: '设备激活多卡',
  card_geo_jump: '异地激活',
};

export default function Blacklist() {
  const [tab, setTab] = useState<'ip' | 'device' | 'alerts'>('alerts');

  const [ips, setIps] = useState<IpEntry[]>([]);
  const [ipTotal, setIpTotal] = useState(0);
  const [ipPage, setIpPage] = useState(1);
  const [ipLoading, setIpLoading] = useState(false);
  const [showIpModal, setShowIpModal] = useState(false);
  const [ipForm, setIpForm] = useState({ ip: '', reason: '' });

  const [devices, setDevices] = useState<DeviceEntry[]>([]);
  const [deviceTotal, setDeviceTotal] = useState(0);
  const [devicePage, setDevicePage] = useState(1);
  const [deviceLoading, setDeviceLoading] = useState(false);
  const [showDeviceModal, setShowDeviceModal] = useState(false);
  const [deviceForm, setDeviceForm] = useState({ device_id: '', reason: '' });

  const [alerts, setAlerts] = useState<AlertEntry[]>([]);
  const [alertTotal, setAlertTotal] = useState(0);
  const [alertPage, setAlertPage] = useState(1);
  const [alertLoading, setAlertLoading] = useState(false);
  const [unreadCount, setUnreadCount] = useState(0);

  const [submitting, setSubmitting] = useState(false);
  const confirm = useConfirm();
  const PAGE_SIZE = 10;

  const loadIps = (p = ipPage) => {
    setIpLoading(true); setIps([]);
    blacklistApi.listIps({ page: p, page_size: PAGE_SIZE })
      .then(r => { if (r.data.success) { setIps(r.data.data); setIpTotal(r.data.total); } })
      .finally(() => setIpLoading(false));
  };

  const loadDevices = (p = devicePage) => {
    setDeviceLoading(true); setDevices([]);
    blacklistApi.listDevices({ page: p, page_size: PAGE_SIZE })
      .then(r => { if (r.data.success) { setDevices(r.data.data); setDeviceTotal(r.data.total); } })
      .finally(() => setDeviceLoading(false));
  };

  const loadAlerts = (p = alertPage) => {
    setAlertLoading(true); setAlerts([]);
    blacklistApi.listAlerts({ page: p, page_size: PAGE_SIZE })
      .then(r => { if (r.data.success) { setAlerts(r.data.data); setAlertTotal(r.data.total); } })
      .finally(() => setAlertLoading(false));
  };

  const loadUnread = () => {
    blacklistApi.unreadAlertCount()
      .then(r => { if (r.data.success) setUnreadCount(r.data.data.unread); })
      .catch(() => {});
  };

  useEffect(() => {
    loadUnread();
    if (tab === 'ip') loadIps(1);
    else if (tab === 'device') loadDevices(1);
    else loadAlerts(1);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  const handleAddIp = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    try {
      const r = await blacklistApi.addIp(ipForm.ip, ipForm.reason || undefined);
      if (r.data.success) { toast.success('已添加'); setShowIpModal(false); setIpForm({ ip: '', reason: '' }); loadIps(1); }
      else toast.error(r.data.message);
    } catch { toast.error('添加失败'); }
    finally { setSubmitting(false); }
  };

  const handleRemoveIp = async (id: string) => {
    const ok = await confirm({ title: '移除 IP', message: '确认从黑名单中移除该 IP？', confirmText: '移除', danger: true });
    if (!ok) return;
    try {
      const r = await blacklistApi.removeIp(id);
      if (r.data.success) { toast.success('已移除'); loadIps(); }
      else toast.error(r.data.message);
    } catch { toast.error('操作失败'); }
  };

  const handleAddDevice = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    try {
      const r = await blacklistApi.addDevice(deviceForm.device_id, deviceForm.reason || undefined);
      if (r.data.success) { toast.success('已添加'); setShowDeviceModal(false); setDeviceForm({ device_id: '', reason: '' }); loadDevices(1); }
      else toast.error(r.data.message);
    } catch { toast.error('添加失败'); }
    finally { setSubmitting(false); }
  };

  const handleRemoveDevice = async (id: string) => {
    const ok = await confirm({ title: '移除设备', message: '确认从黑名单中移除该设备？', confirmText: '移除', danger: true });
    if (!ok) return;
    try {
      const r = await blacklistApi.removeDevice(id);
      if (r.data.success) { toast.success('已移除'); loadDevices(); }
      else toast.error(r.data.message);
    } catch { toast.error('操作失败'); }
  };

  const handleMarkRead = async (id: string) => {
    try {
      await blacklistApi.markAlertRead(id);
      setAlerts(prev => prev.map(a => a.id === id ? { ...a, is_read: true } : a));
      setUnreadCount(c => Math.max(0, c - 1));
    } catch { /* ignore */ }
  };

  const ipPages = Math.ceil(ipTotal / PAGE_SIZE);
  const devicePages = Math.ceil(deviceTotal / PAGE_SIZE);
  const alertPages = Math.ceil(alertTotal / PAGE_SIZE);

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">风控管理</h1>
          <p className="page-subtitle">IP / 设备黑名单与异常激活告警</p>
        </div>
      </div>

      {/* Tab 切换 */}
      <div style={{ display: 'flex', gap: 4, marginBottom: 20, borderBottom: '1px solid var(--border)', paddingBottom: 0 }}>
        {(['alerts', 'ip', 'device'] as const).map(t => {
          const labels = { alerts: '异常告警', ip: 'IP 黑名单', device: '设备黑名单' };
          const icons = { alerts: <AlertTriangle size={14} />, ip: <Shield size={14} />, device: <Monitor size={14} /> };
          return (
            <button key={t} onClick={() => setTab(t)} style={{
              padding: '8px 16px', border: 'none', cursor: 'pointer', fontSize: 13,
              borderBottom: tab === t ? '2px solid var(--accent)' : '2px solid transparent',
              color: tab === t ? 'var(--accent)' : 'var(--text-muted)',
              background: 'none', display: 'flex', alignItems: 'center', gap: 6,
            }}>
              {icons[t]} {labels[t]}
              {t === 'alerts' && unreadCount > 0 && (
                <span style={{ background: '#f87171', color: '#fff', borderRadius: 10, padding: '1px 6px', fontSize: 11 }}>
                  {unreadCount}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {/* 异常告警 */}
      {tab === 'alerts' && (
        <>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginBottom: 12 }}>
            <button className="btn btn-ghost" onClick={() => loadAlerts()}><RefreshCw size={14} /> 刷新</button>
          </div>
          <div className="table-wrap">
            <table>
              <thead><tr><th>告警类型</th><th>设备</th><th>IP</th><th>详情</th><th>时间</th><th>状态</th></tr></thead>
              <tbody>
                {alertLoading ? Array.from({ length: 5 }).map((_, i) => (
                  <tr key={i} className="skeleton-row">
                    {Array.from({ length: 6 }).map((_, j) => <td key={j}><span className="skeleton" style={{ width: '60%' }} /></td>)}
                  </tr>
                )) : alerts.length === 0 ? (
                  <tr><td colSpan={6}><div className="empty-state"><div className="empty-state-icon">✅</div><div className="empty-state-text">暂无异常告警</div></div></td></tr>
                ) : alerts.map((a, idx) => (
                  <tr key={a.id} className="data-enter" style={{ animationDelay: `${idx * 30}ms`, opacity: a.is_read ? 0.6 : 1 }}>
                    <td>
                      <span style={{
                        padding: '2px 8px', borderRadius: 12, fontSize: 12,
                        background: a.alert_type === 'ip_abuse' ? 'rgba(251,191,36,0.15)' : 'rgba(248,113,113,0.15)',
                        color: a.alert_type === 'ip_abuse' ? '#fbbf24' : '#f87171',
                      }}>
                        {ALERT_LABELS[a.alert_type] || a.alert_type}
                      </span>
                    </td>
                    <td><span className="mono" style={{ fontSize: 12 }}>{a.device_hint || '—'}</span></td>
                    <td><span className="mono" style={{ fontSize: 12 }}>{a.ip_address || '—'}</span></td>
                    <td><span style={{ color: 'var(--text-muted)', fontSize: 12 }}>{a.detail || '—'}</span></td>
                    <td><span style={{ fontSize: 12 }}>{new Date(a.created_at).toLocaleString('zh-CN')}</span></td>
                    <td>
                      {a.is_read ? (
                        <span style={{ color: 'var(--text-muted)', fontSize: 12 }}>已读</span>
                      ) : (
                        <button className="btn btn-sm btn-ghost" style={{ color: '#34d399', borderColor: 'rgba(52,211,153,0.3)' }}
                          onClick={() => handleMarkRead(a.id)}><CheckCircle size={12} /></button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {alertPages > 1 && (
            <div className="pagination">
              {Array.from({ length: alertPages }, (_, i) => i + 1).map(p => (
                <button key={p} className={`page-btn ${p === alertPage ? 'active' : ''}`}
                  onClick={() => { setAlertPage(p); loadAlerts(p); }}>{p}</button>
              ))}
            </div>
          )}
        </>
      )}

      {/* IP 黑名单 */}
      {tab === 'ip' && (
        <>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginBottom: 12 }}>
            <button className="btn btn-ghost" onClick={() => loadIps()}><RefreshCw size={14} /> 刷新</button>
            <button className="btn btn-primary" onClick={() => setShowIpModal(true)}><Plus size={14} /> 添加 IP</button>
          </div>
          <div className="table-wrap">
            <table>
              <thead><tr><th>IP 地址</th><th>原因</th><th>添加时间</th><th>操作</th></tr></thead>
              <tbody>
                {ipLoading ? Array.from({ length: 5 }).map((_, i) => (
                  <tr key={i} className="skeleton-row">
                    <td><span className="skeleton" style={{ width: '60%' }} /></td>
                    <td><span className="skeleton" style={{ width: '70%' }} /></td>
                    <td><span className="skeleton" style={{ width: '50%' }} /></td>
                    <td><span className="skeleton" style={{ width: 32, height: 28 }} /></td>
                  </tr>
                )) : ips.length === 0 ? (
                  <tr><td colSpan={4}><div className="empty-state"><div className="empty-state-icon">🛡️</div><div className="empty-state-text">暂无 IP 黑名单</div></div></td></tr>
                ) : ips.map((item, idx) => (
                  <tr key={item.id} className="data-enter" style={{ animationDelay: `${idx * 30}ms` }}>
                    <td><span className="mono" style={{ color: 'var(--accent)' }}>{item.ip}</span></td>
                    <td><span style={{ color: 'var(--text-muted)' }}>{item.reason || '—'}</span></td>
                    <td>{new Date(item.created_at).toLocaleString('zh-CN')}</td>
                    <td><button className="btn btn-sm btn-danger" onClick={() => handleRemoveIp(item.id)}><Trash2 size={12} /></button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {ipPages > 1 && (
            <div className="pagination">
              {Array.from({ length: ipPages }, (_, i) => i + 1).map(p => (
                <button key={p} className={`page-btn ${p === ipPage ? 'active' : ''}`}
                  onClick={() => { setIpPage(p); loadIps(p); }}>{p}</button>
              ))}
            </div>
          )}
        </>
      )}

      {/* 设备黑名单 */}
      {tab === 'device' && (
        <>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginBottom: 12 }}>
            <button className="btn btn-ghost" onClick={() => loadDevices()}><RefreshCw size={14} /> 刷新</button>
            <button className="btn btn-primary" onClick={() => setShowDeviceModal(true)}><Plus size={14} /> 添加设备</button>
          </div>
          <div className="table-wrap">
            <table>
              <thead><tr><th>设备标识</th><th>原因</th><th>添加时间</th><th>操作</th></tr></thead>
              <tbody>
                {deviceLoading ? Array.from({ length: 5 }).map((_, i) => (
                  <tr key={i} className="skeleton-row">
                    <td><span className="skeleton" style={{ width: '55%' }} /></td>
                    <td><span className="skeleton" style={{ width: '70%' }} /></td>
                    <td><span className="skeleton" style={{ width: '50%' }} /></td>
                    <td><span className="skeleton" style={{ width: 32, height: 28 }} /></td>
                  </tr>
                )) : devices.length === 0 ? (
                  <tr><td colSpan={4}><div className="empty-state"><div className="empty-state-icon">💻</div><div className="empty-state-text">暂无设备黑名单</div></div></td></tr>
                ) : devices.map((item, idx) => (
                  <tr key={item.id} className="data-enter" style={{ animationDelay: `${idx * 30}ms` }}>
                    <td><span className="mono" style={{ color: 'var(--accent)' }}>{item.device_hint || '—'}</span></td>
                    <td><span style={{ color: 'var(--text-muted)' }}>{item.reason || '—'}</span></td>
                    <td>{new Date(item.created_at).toLocaleString('zh-CN')}</td>
                    <td><button className="btn btn-sm btn-danger" onClick={() => handleRemoveDevice(item.id)}><Trash2 size={12} /></button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {devicePages > 1 && (
            <div className="pagination">
              {Array.from({ length: devicePages }, (_, i) => i + 1).map(p => (
                <button key={p} className={`page-btn ${p === devicePage ? 'active' : ''}`}
                  onClick={() => { setDevicePage(p); loadDevices(p); }}>{p}</button>
              ))}
            </div>
          )}
        </>
      )}

      {/* 添加 IP 弹窗 */}
      {showIpModal && (
        <div className="modal-overlay" onClick={() => setShowIpModal(false)}>
          <div className="modal" style={{ maxWidth: 400 }} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">添加 IP 黑名单</h2>
            <form onSubmit={handleAddIp}>
              <div className="form-group">
                <label className="form-label">IP 地址 *</label>
                <input placeholder="如：192.168.1.1" value={ipForm.ip}
                  onChange={e => setIpForm({ ...ipForm, ip: e.target.value })} required />
              </div>
              <div className="form-group">
                <label className="form-label">原因（可选）</label>
                <input placeholder="如：频繁激活" value={ipForm.reason}
                  onChange={e => setIpForm({ ...ipForm, reason: e.target.value })} />
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => setShowIpModal(false)}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting}>
                  {submitting ? <span className="spinner" /> : '添加'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 添加设备弹窗 */}
      {showDeviceModal && (
        <div className="modal-overlay" onClick={() => setShowDeviceModal(false)}>
          <div className="modal" style={{ maxWidth: 400 }} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">添加设备黑名单</h2>
            <form onSubmit={handleAddDevice}>
              <div className="form-group">
                <label className="form-label">设备 ID *</label>
                <input placeholder="输入完整的设备 ID" value={deviceForm.device_id}
                  onChange={e => setDeviceForm({ ...deviceForm, device_id: e.target.value })} required />
                <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>存储时自动哈希，页面仅显示脱敏标识</span>
              </div>
              <div className="form-group">
                <label className="form-label">原因（可选）</label>
                <input placeholder="如：黄牛设备" value={deviceForm.reason}
                  onChange={e => setDeviceForm({ ...deviceForm, reason: e.target.value })} />
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => setShowDeviceModal(false)}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting}>
                  {submitting ? <span className="spinner" /> : '添加'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
} 