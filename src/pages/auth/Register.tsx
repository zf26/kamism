import { useState } from 'react';
import { useNavigate, Link } from 'react-router-dom';
import { authApi } from '../../lib/api';
import { Mail, Lock, User, ArrowRight, ShieldCheck } from 'lucide-react';
import appIcon from '../../assets/app-icon.png';
import toast from 'react-hot-toast';

export default function Register() {
  const navigate = useNavigate();
  const [form, setForm] = useState({ username: '', email: '', password: '', confirm: '', code: '' });
  const [loading, setLoading] = useState(false);
  const [codeSending, setCodeSending] = useState(false);
  const [countdown, setCountdown] = useState(0);

  const startCountdown = () => {
    setCountdown(60);
    const timer = setInterval(() => {
      setCountdown(prev => {
        if (prev <= 1) { clearInterval(timer); return 0; }
        return prev - 1;
      });
    }, 1000);
  };

  const handleSendCode = async () => {
    if (!form.email.includes('@')) { toast.error('请输入正确的邮箱'); return; }
    setCodeSending(true);
    try {
      const res = await authApi.sendCode(form.email);
      if (res.data.success) {
        toast.success('验证码已发送，请查收邮件');
        startCountdown();
      } else {
        toast.error(res.data.message || '发送失败');
      }
    } catch {
      toast.error('发送失败，请检查网络');
    } finally {
      setCodeSending(false);
    }
  };

  const handleRegister = async (e: React.FormEvent) => {
    e.preventDefault();
    if (form.password !== form.confirm) { toast.error('两次密码不一致'); return; }
    if (!form.code) { toast.error('请输入验证码'); return; }
    setLoading(true);
    try {
      const res = await authApi.register({
        username: form.username,
        email: form.email,
        password: form.password,
        code: form.code,
      });
      if (res.data.success) {
        toast.success('注册成功，请登录');
        navigate('/login');
      } else {
        toast.error(res.data.message || '注册失败');
      }
    } catch {
      toast.error('注册失败，请检查网络');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div style={{
      minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center',
      background: 'radial-gradient(ellipse 80% 60% at 50% -20%, rgba(124,106,247,0.15), transparent)',
    }}>
      <div style={{ width: '100%', maxWidth: 420, padding: '0 20px' }}>
        <div style={{ textAlign: 'center', marginBottom: 40 }}>
          <img
            src={appIcon}
            alt="KamiSM"
            style={{ width: 52, height: 52, margin: '0 auto 16px', display: 'block', borderRadius: 14, boxShadow: '0 8px 32px rgba(124,106,247,0.3)' }}
          />
          <h1 style={{ fontSize: 26, fontWeight: 800, letterSpacing: '-0.5px' }}>创建账号</h1>
          <p style={{ color: 'var(--text-muted)', fontSize: 13, marginTop: 4 }}>注册商户账号，开始使用 KamiSM</p>
        </div>

        <div className="card" style={{ padding: 32 }}>
          <form onSubmit={handleRegister}>
            {/* 用户名 */}
            <div className="form-group">
              <label className="form-label">用户名</label>
              <div style={{ position: 'relative' }}>
                <User size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                <input type="text" value={form.username} onChange={e => setForm({ ...form, username: e.target.value })}
                  placeholder="至少3位" required style={{ paddingLeft: 36 }} />
              </div>
            </div>

            {/* 邮箱 + 发送验证码 */}
            <div className="form-group">
              <label className="form-label">邮箱</label>
              <div style={{ display: 'flex', gap: 8 }}>
                <div style={{ position: 'relative', flex: 1 }}>
                  <Mail size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                  <input type="email" value={form.email} onChange={e => setForm({ ...form, email: e.target.value })}
                    placeholder="your@email.com" required style={{ paddingLeft: 36 }} />
                </div>
                <button
                  type="button"
                  className="btn btn-ghost"
                  style={{ flexShrink: 0, minWidth: 96, fontSize: 12 }}
                  onClick={handleSendCode}
                  disabled={codeSending || countdown > 0}
                >
                  {codeSending ? <span className="spinner" /> : countdown > 0 ? `${countdown}s` : '发送验证码'}
                </button>
              </div>
            </div>

            {/* 验证码 */}
            <div className="form-group">
              <label className="form-label">验证码</label>
              <div style={{ position: 'relative' }}>
                <ShieldCheck size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                <input
                  type="text" value={form.code} onChange={e => setForm({ ...form, code: e.target.value.replace(/\D/g, '').slice(0, 6) })}
                  placeholder="6位数字验证码" required maxLength={6}
                  style={{ paddingLeft: 36, letterSpacing: '6px', fontFamily: 'var(--mono)', fontSize: 16 }}
                />
              </div>
            </div>

            {/* 密码 */}
            <div className="form-group">
              <label className="form-label">密码</label>
              <div style={{ position: 'relative' }}>
                <Lock size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                <input type="password" value={form.password} onChange={e => setForm({ ...form, password: e.target.value })}
                  placeholder="至少8位" required style={{ paddingLeft: 36 }} />
              </div>
            </div>

            {/* 确认密码 */}
            <div className="form-group">
              <label className="form-label">确认密码</label>
              <div style={{ position: 'relative' }}>
                <Lock size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                <input type="password" value={form.confirm} onChange={e => setForm({ ...form, confirm: e.target.value })}
                  placeholder="再次输入密码" required style={{ paddingLeft: 36 }} />
              </div>
            </div>

            <button type="submit" className="btn btn-primary"
              style={{ width: '100%', justifyContent: 'center', marginTop: 8, padding: '12px' }}
              disabled={loading}>
              {loading ? <span className="spinner" /> : <><span>注册</span><ArrowRight size={15} /></>}
            </button>
          </form>

          <div style={{ textAlign: 'center', marginTop: 20, color: 'var(--text-muted)', fontSize: 13 }}>
            已有账号？
            <Link to="/login" style={{ color: 'var(--accent)', textDecoration: 'none', fontWeight: 600, marginLeft: 4 }}>立即登录</Link>
          </div>
        </div>
      </div>
    </div>
  );
}
