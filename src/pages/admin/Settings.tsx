import { useState } from 'react';
import { useAuthStore } from '../../stores/auth';
import { adminApi } from '../../lib/api';
import { Lock, ShieldCheck, User } from 'lucide-react';
import toast from 'react-hot-toast';

/**
 * 管理端账号设置。
 *
 * 改造前后台**没有**修改密码的入口 —— 管理员忘记密码或怀疑泄露时，
 * 唯一的办法是直接连库改 `admins.password_hash`。
 * 而管理员能做的事包括给任意商户开专业版、改支付渠道密钥，
 * 这类账号反而最需要一个自助改密的入口。
 */
export default function AdminSettings() {
  const { user, updateToken } = useAuthStore();
  const [form, setForm] = useState({ old_password: '', new_password: '', confirm: '' });
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (form.new_password !== form.confirm) { toast.error('两次密码不一致'); return; }
    if (form.new_password.length < 8) { toast.error('新密码至少 8 位'); return; }
    if (form.new_password === form.old_password) { toast.error('新密码不能与原密码相同'); return; }

    setLoading(true);
    try {
      const res = await adminApi.changeAdminPassword({
        old_password: form.old_password,
        new_password: form.new_password,
      });
      if (res.data.success) {
        // 后端改密码时会吊销该管理员的**全部**令牌（含当前这台设备），
        // 同时在响应里补发了一对新令牌 —— 必须存下来，否则下一次请求就 401。
        const { token, refresh_token } = res.data;
        if (token && refresh_token) {
          updateToken(token, refresh_token);
          toast.success('密码已修改，其他设备需重新登录');
        } else {
          toast.success('密码已修改，请重新登录');
        }
        setForm({ old_password: '', new_password: '', confirm: '' });
      } else {
        toast.error(res.data.message || '修改失败');
      }
    } catch {
      toast.error('操作失败，请检查网络');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">账号设置</h1>
          <p className="page-subtitle">管理员账号安全</p>
        </div>
      </div>

      <div style={{ display: 'grid', gap: 20, maxWidth: 600 }}>
        <div className="card">
          <p style={{ fontWeight: 700, marginBottom: 16, color: 'var(--text)', display: 'flex', alignItems: 'center', gap: 6 }}>
            <User size={15} /> 账号信息
          </p>
          <div style={{ display: 'grid', gap: 10 }}>
            {[
              { label: '用户名', value: user?.username ?? '-' },
              { label: '邮箱', value: user?.email ?? '-' },
              { label: '账号类型', value: '平台管理员' },
            ].map(item => (
              <div key={item.label} style={{ display: 'flex', justifyContent: 'space-between', padding: '10px 0', borderBottom: '1px solid var(--border)' }}>
                <span style={{ color: 'var(--text-muted)', fontSize: 13 }}>{item.label}</span>
                <span style={{ color: 'var(--text)', fontWeight: 500 }}>{item.value}</span>
              </div>
            ))}
          </div>
        </div>

        <div className="card">
          <p style={{ fontWeight: 700, color: 'var(--text)', display: 'flex', alignItems: 'center', gap: 6, marginBottom: 8 }}>
            <Lock size={15} /> 修改密码
          </p>
          <p style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 16, lineHeight: 1.7 }}>
            修改后<strong style={{ color: 'var(--text)' }}>该账号在所有设备上的登录都会失效</strong>，
            当前这台会自动换成新令牌，其他设备需要重新登录。
          </p>
          <form onSubmit={handleSubmit}>
            {[
              { key: 'old_password', label: '当前密码', placeholder: '输入当前密码' },
              { key: 'new_password', label: '新密码', placeholder: '至少 8 位' },
              { key: 'confirm', label: '确认新密码', placeholder: '再次输入新密码' },
            ].map(f => (
              <div className="form-group" key={f.key}>
                <label className="form-label">{f.label}</label>
                <input
                  type="password"
                  placeholder={f.placeholder}
                  value={form[f.key as keyof typeof form]}
                  onChange={e => setForm({ ...form, [f.key]: e.target.value })}
                  required
                />
              </div>
            ))}
            <button type="submit" className="btn btn-primary" disabled={loading}>
              {loading ? <span className="spinner" /> : <><ShieldCheck size={14} /> 修改密码</>}
            </button>
          </form>
        </div>
      </div>
    </div>
  );
}
