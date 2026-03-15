import { useState } from 'react';
import { useNavigate, Link } from 'react-router-dom';
import { useAuthStore } from '../../stores/auth';
import { authApi } from '../../lib/api';
import { Mail, Lock, ArrowRight } from 'lucide-react';
import toast from 'react-hot-toast';
import appIcon from '../../assets/app-icon.png';

export default function Login() {
  const navigate = useNavigate();
  const { setAuth } = useAuthStore();
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [loading, setLoading] = useState(false);

  const handleLogin = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    try {
      const res = await authApi.login({ email, password });
      const { success, token, refresh_token, role, user, message } = res.data;
      if (success) {
        setAuth(token, refresh_token, role, user);
        toast.success('登录成功');
        navigate(role === 'admin' ? '/admin/dashboard' : '/dashboard');
      } else {
        toast.error(message || '登录失败');
      }
    } catch {
      toast.error('登录失败，请检查网络');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div style={{
      minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center',
      background: 'radial-gradient(ellipse 80% 60% at 50% -20%, rgba(124,106,247,0.15), transparent)',
    }}>
      <div style={{ width: '100%', maxWidth: 400, padding: '0 20px' }}>
        {/* Logo */}
        <div style={{ textAlign: 'center', marginBottom: 40 }}>
          <img
            src={appIcon}
            alt="KamiSM"
            style={{ width: 52, height: 52, margin: '0 auto 16px', display: 'block', borderRadius: 14, boxShadow: '0 8px 32px rgba(124,106,247,0.3)' }}
          />
          <h1 style={{ fontSize: 26, fontWeight: 800, letterSpacing: '-0.5px' }}>KamiSM</h1>
          <p style={{ color: 'var(--text-muted)', fontSize: 13, marginTop: 4 }}>卡密管理平台 · 商户登录</p>
        </div>

        <div className="card" style={{ padding: 32 }}>
          <form onSubmit={handleLogin}>
            <div className="form-group">
              <label className="form-label">邮箱</label>
              <div style={{ position: 'relative' }}>
                <Mail size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                <input
                  type="email" value={email} onChange={e => setEmail(e.target.value)}
                  placeholder="your@email.com" required
                  style={{ paddingLeft: 36 }}
                />
              </div>
            </div>
            <div className="form-group">
              <label className="form-label">密码</label>
              <div style={{ position: 'relative' }}>
                <Lock size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                <input
                  type="password" value={password} onChange={e => setPassword(e.target.value)}
                  placeholder="••••••••" required
                  style={{ paddingLeft: 36 }}
                />
              </div>
            </div>
            <button type="submit" className="btn btn-primary" style={{ width: '100%', justifyContent: 'center', marginTop: 8, padding: '12px' }} disabled={loading}>
              {loading ? <span className="spinner" /> : <><span>登录</span><ArrowRight size={15} /></>}
            </button>
          </form>

          <div style={{ textAlign: 'center', marginTop: 20, color: 'var(--text-muted)', fontSize: 13 }}>
            没有账号？
            <Link to="/register" style={{ color: 'var(--accent)', textDecoration: 'none', fontWeight: 600, marginLeft: 4 }}>立即注册</Link>
          </div>
        </div>
      </div>
    </div>
  );
}

