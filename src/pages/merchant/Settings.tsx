import { useState } from 'react';
import { useAuthStore } from '../../stores/auth';
import { merchantApi } from '../../lib/api';
import { Copy, RefreshCw, Lock, Key } from 'lucide-react';
import toast from 'react-hot-toast';
import { useConfirm } from '../../stores/confirm';

export default function Settings() {
  const { user, setAuth, role } = useAuthStore();
  const [pwForm, setPwForm] = useState({ old_password: '', new_password: '', confirm: '' });
  const [pwLoading, setPwLoading] = useState(false);
  const [apiLoading, setApiLoading] = useState(false);
  const [apiKey, setApiKey] = useState(user?.api_key || '');

  const copyApiKey = () => {
    navigator.clipboard.writeText(apiKey);
    toast.success('API Key 已复制');
  };

  const handleChangePassword = async (e: React.FormEvent) => {
    e.preventDefault();
    if (pwForm.new_password !== pwForm.confirm) { toast.error('两次密码不一致'); return; }
    if (pwForm.new_password.length < 8) { toast.error('新密码至少8位'); return; }
    setPwLoading(true);
    try {
      const res = await merchantApi.changePassword({
        old_password: pwForm.old_password,
        new_password: pwForm.new_password,
      });
      if (res.data.success) {
        toast.success('密码已修改，请重新登录');
        setPwForm({ old_password: '', new_password: '', confirm: '' });
      } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
    finally { setPwLoading(false); }
  };

  const confirm = useConfirm();

  const handleRegenerateApiKey = async () => {
    const ok = await confirm({ title: '重新生成 API Key', message: '旧 Key 将立即失效，已集成的软件需同步更新。确认继续？', confirmText: '重新生成', danger: true });
    if (!ok) return;
    setApiLoading(true);
    try {
      const res = await merchantApi.regenerateApiKey();
      if (res.data.success) {
        const newKey = res.data.data.api_key;
        setApiKey(newKey);
        if (user) {
          const token = localStorage.getItem('token') || '';
          const refreshToken = localStorage.getItem('refreshToken') || '';
          setAuth(token, refreshToken, role!, { ...user, api_key: newKey });
        }
        toast.success('API Key 已更新');
      } else toast.error(res.data.message);
    } catch { toast.error('操作失败'); }
    finally { setApiLoading(false); }
  };

  return (
    <div className="fade-in">
      <div className="page-header">
        <div><h1 className="page-title">账号设置</h1><p className="page-subtitle">管理你的账号信息</p></div>
      </div>

      <div style={{ display: 'grid', gap: 20, maxWidth: 600 }}>
        {/* 账号信息 */}
        <div className="card">
          <p style={{ fontWeight: 700, marginBottom: 16, color: 'var(--text)' }}>账号信息</p>
          <div style={{ display: 'grid', gap: 10 }}>
            {[{ label: '用户名', value: user?.username }, { label: '邮箱', value: user?.email }].map(item => (
              <div key={item.label} style={{ display: 'flex', justifyContent: 'space-between', padding: '10px 0', borderBottom: '1px solid var(--border)' }}>
                <span style={{ color: 'var(--text-muted)', fontSize: 13 }}>{item.label}</span>
                <span style={{ color: 'var(--text)', fontWeight: 500 }}>{item.value}</span>
              </div>
            ))}
          </div>
        </div>

        {/* API Key */}
        <div className="card">
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
            <p style={{ fontWeight: 700, color: 'var(--text)', display: 'flex', alignItems: 'center', gap: 6 }}><Key size={15} /> API Key</p>
          </div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', background: 'var(--bg)', border: '1px solid var(--border)', borderRadius: 8, padding: '10px 14px', marginBottom: 12 }}>
            <span className="mono" style={{ flex: 1, fontSize: 13, color: 'var(--accent)', wordBreak: 'break-all' }}>{apiKey}</span>
            <button style={{ background: 'none', color: 'var(--text-muted)', padding: 4, flexShrink: 0 }} onClick={copyApiKey}><Copy size={15} /></button>
          </div>
          <p style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 12 }}>在你的软件中使用此 Key 调用 KamiSM API 接口</p>
          <button className="btn btn-ghost btn-sm" onClick={handleRegenerateApiKey} disabled={apiLoading}>
            {apiLoading ? <span className="spinner" /> : <><RefreshCw size={13} /> 重新生成 Key</>}
          </button>
        </div>

        {/* 修改密码 */}
        <div className="card">
          <p style={{ fontWeight: 700, color: 'var(--text)', display: 'flex', alignItems: 'center', gap: 6, marginBottom: 16 }}><Lock size={15} /> 修改密码</p>
          <form onSubmit={handleChangePassword}>
            {[
              { key: 'old_password', label: '当前密码', placeholder: '输入当前密码' },
              { key: 'new_password', label: '新密码', placeholder: '至少8位' },
              { key: 'confirm', label: '确认新密码', placeholder: '再次输入新密码' },
            ].map(f => (
              <div className="form-group" key={f.key}>
                <label className="form-label">{f.label}</label>
                <input type="password" placeholder={f.placeholder} value={pwForm[f.key as keyof typeof pwForm]}
                  onChange={e => setPwForm({ ...pwForm, [f.key]: e.target.value })} required />
              </div>
            ))}
            <button type="submit" className="btn btn-primary" disabled={pwLoading}>
              {pwLoading ? <span className="spinner" /> : '修改密码'}
            </button>
          </form>
        </div>
      </div>
    </div>
  );
}

