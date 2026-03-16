import { useState } from 'react';
import { useNavigate, Link } from 'react-router-dom';
import { authApi } from '../../lib/api';
import { Mail, Lock, ArrowRight, ShieldCheck } from 'lucide-react';
import appIcon from '../../assets/app-icon.png';
import toast from 'react-hot-toast';

export default function ResetPassword() {
  const navigate = useNavigate();
  const [step, setStep] = useState<'email' | 'code' | 'password'>('email');
  const [form, setForm] = useState({ email: '', code: '', password: '', confirm: '' });
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
      const res = await authApi.sendResetCode(form.email);
      if (res.data.success) {
        toast.success('验证码已发送，请查收邮件');
        setStep('code');
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

  const handleVerifyCode = () => {
    if (!form.code || form.code.length !== 6) { toast.error('请输入6位验证码'); return; }
    setStep('password');
  };

  const handleResetPassword = async (e: React.FormEvent) => {
    e.preventDefault();
    if (form.password.length < 8) { toast.error('密码至少8位'); return; }
    if (form.password !== form.confirm) { toast.error('两次密码不一致'); return; }
    setLoading(true);
    try {
      const res = await authApi.resetPassword({
        email: form.email,
        code: form.code,
        new_password: form.password,
      });
      if (res.data.success) {
        toast.success('密码重置成功，请重新登录');
        navigate('/login');
      } else {
        toast.error(res.data.message || '重置失败');
      }
    } catch {
      toast.error('重置失败，请检查网络');
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
          <h1 style={{ fontSize: 26, fontWeight: 800, letterSpacing: '-0.5px' }}>重置密码</h1>
          <p style={{ color: 'var(--text-muted)', fontSize: 13, marginTop: 4 }}>
            {step === 'email' && '输入注册邮箱，我们将发送验证码'}
            {step === 'code' && '输入邮件中的验证码'}
            {step === 'password' && '设置新密码'}
          </p>
        </div>

        <div className="card" style={{ padding: 32 }}>
          {/* 第一步：输入邮箱 */}
          {step === 'email' && (
            <form onSubmit={e => { e.preventDefault(); handleSendCode(); }}>
              <div className="form-group">
                <label className="form-label">邮箱</label>
                <div style={{ position: 'relative' }}>
                  <Mail size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                  <input
                    type="email" value={form.email} onChange={e => setForm({ ...form, email: e.target.value })}
                    placeholder="your@email.com" required style={{ paddingLeft: 36 }}
                  />
                </div>
              </div>
              <button type="submit" className="btn btn-primary"
                style={{ width: '100%', justifyContent: 'center', marginTop: 8, padding: '12px' }}
                disabled={codeSending}>
                {codeSending ? <span className="spinner" /> : <><span>发送验证码</span><ArrowRight size={15} /></>}
              </button>
            </form>
          )}

          {/* 第二步：输入验证码 */}
          {step === 'code' && (
            <form onSubmit={e => { e.preventDefault(); handleVerifyCode(); }}>
              <div className="form-group">
                <label className="form-label">验证码</label>
                <p style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 8 }}>已发送至 {form.email}</p>
                <div style={{ position: 'relative' }}>
                  <ShieldCheck size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                  <input
                    type="text" value={form.code} onChange={e => setForm({ ...form, code: e.target.value.replace(/\D/g, '').slice(0, 6) })}
                    placeholder="6位数字验证码" required maxLength={6}
                    style={{ paddingLeft: 36, letterSpacing: '6px', fontFamily: 'var(--mono)', fontSize: 16 }}
                  />
                </div>
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <button type="button" className="btn btn-ghost"
                  style={{ flex: 1, padding: '12px' }}
                  onClick={() => setStep('email')}>
                  返回
                </button>
                <button type="submit" className="btn btn-primary"
                  style={{ flex: 1, justifyContent: 'center', padding: '12px' }}>
                  <><span>下一步</span><ArrowRight size={15} /></>
                </button>
              </div>
              <div style={{ textAlign: 'center', marginTop: 12, fontSize: 12, color: 'var(--text-muted)' }}>
                {countdown > 0 ? `${countdown}秒后可重新发送` : (
                  <button type="button" style={{ background: 'none', border: 'none', color: 'var(--accent)', cursor: 'pointer', fontSize: 12 }}
                    onClick={handleSendCode} disabled={codeSending}>
                    重新发送验证码
                  </button>
                )}
              </div>
            </form>
          )}

          {/* 第三步：设置新密码 */}
          {step === 'password' && (
            <form onSubmit={handleResetPassword}>
              <div className="form-group">
                <label className="form-label">新密码</label>
                <div style={{ position: 'relative' }}>
                  <Lock size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                  <input
                    type="password" value={form.password} onChange={e => setForm({ ...form, password: e.target.value })}
                    placeholder="至少8位" required style={{ paddingLeft: 36 }}
                  />
                </div>
              </div>

              <div className="form-group">
                <label className="form-label">确认密码</label>
                <div style={{ position: 'relative' }}>
                  <Lock size={15} style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
                  <input
                    type="password" value={form.confirm} onChange={e => setForm({ ...form, confirm: e.target.value })}
                    placeholder="再次输入密码" required style={{ paddingLeft: 36 }}
                  />
                </div>
              </div>

              <div style={{ display: 'flex', gap: 8 }}>
                <button type="button" className="btn btn-ghost"
                  style={{ flex: 1, padding: '12px' }}
                  onClick={() => setStep('code')}>
                  返回
                </button>
                <button type="submit" className="btn btn-primary"
                  style={{ flex: 1, justifyContent: 'center', padding: '12px' }}
                  disabled={loading}>
                  {loading ? <span className="spinner" /> : <><span>重置密码</span><ArrowRight size={15} /></>}
                </button>
              </div>
            </form>
          )}

          <div style={{ textAlign: 'center', marginTop: 20, color: 'var(--text-muted)', fontSize: 13 }}>
            记得密码了？
            <Link to="/login" style={{ color: 'var(--accent)', textDecoration: 'none', fontWeight: 600, marginLeft: 4 }}>立即登录</Link>
          </div>
        </div>
      </div>
    </div>
  );
}

