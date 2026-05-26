import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuthStore } from '../../stores/auth';
import { paymentsApi } from '../../lib/api';
import toast from 'react-hot-toast';
import { CreditCard, CheckCircle, XCircle, Clock, RefreshCw } from 'lucide-react';

type PayStatus = 'pending' | 'paid' | 'expired';
type PayType = 'wechat' | 'alipay';

interface PlanOption {
  label: string;
  price: string;
  days: number | null;
  badge?: string;
  highlight?: boolean;
}

const PLAN_OPTIONS: PlanOption[] = [
  { label: '30 天续费', price: '30.00', days: 30 },
  { label: '90 天续费', price: '90.00', days: 90, badge: '省 9 元' },
  { label: '180 天续费', price: '180.00', days: 180, badge: '省 18 元' },
  { label: '永久专业版', price: '365.00', days: null, highlight: true, badge: '最划算' },
];

export default function Upgrade() {
  const navigate = useNavigate();
  const { user, updateUser } = useAuthStore();

  // 选中套餐
  const [selectedPlan, setSelectedPlan] = useState<PlanOption | null>(
    PLAN_OPTIONS.find(p => p.highlight) ?? null
  );
  const [payType, setPayType] = useState<PayType>('wechat');
  const [creating, setCreating] = useState(false);

  // 当前订单
  const [currentOrder, setCurrentOrder] = useState<{
    order_id: string;
    qr_url: string;
    pay_type: string;
    price: string;
    status: PayStatus;
    expires_in: number;
    expires_days: number | null;
    plan: string;
  } | null>(null);

  // 轮询 timer
  const pollTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const clearPoll = () => {
    if (pollTimerRef.current) {
      clearInterval(pollTimerRef.current);
      pollTimerRef.current = null;
    }
  };

  // 启动轮询查单状态
  const startPoll = (orderId: string) => {
    clearPoll();
    pollTimerRef.current = setInterval(async () => {
      try {
        const res = await paymentsApi.getStatus(orderId);
        if (res.data.success) {
          const d = res.data.data;
          if (d.status === 'paid') {
            clearPoll();
            setCurrentOrder(prev => prev ? { ...prev, status: 'paid' } : null);
            toast.success('支付成功！专业版已开通');
            // 刷新用户状态
            setTimeout(() => {
              const stored = localStorage.getItem('user');
              if (stored) {
                const u = JSON.parse(stored);
                updateUser({ ...u, plan: 'pro' });
              }
              navigate('/dashboard');
            }, 1500);
          } else if (d.status === 'expired') {
            clearPoll();
            setCurrentOrder(prev => prev ? { ...prev, status: 'expired' } : null);
          }
        }
      } catch { /* ignore */ }
    }, 3000);
  };

  useEffect(() => {
    return () => clearPoll();
  }, []);

  const handleCreateOrder = async () => {
    if (!selectedPlan) { toast.error('请先选择套餐'); return; }
    setCreating(true);
    try {
      const res = await paymentsApi.create({
        pay_type: payType,
        plan: 'pro',
        expires_days: selectedPlan.days ?? undefined,
      });
      if (res.data.success) {
        const d = res.data.data;
        setCurrentOrder({
          order_id: d.order_id,
          qr_url: d.qr_url,
          pay_type: d.pay_type,
          price: d.price,
          status: 'pending',
          expires_in: d.expires_in,
          expires_days: d.expires_days,
          plan: d.plan,
        });
        startPoll(d.order_id);
      } else {
        toast.error(res.data.message || '创建订单失败');
      }
    } catch {
      toast.error('网络错误，请稍后重试');
    } finally {
      setCreating(false);
    }
  };

  const handleCancelOrder = () => {
    clearPoll();
    setCurrentOrder(null);
  };

  const StatusBadge = ({ status }: { status: PayStatus }) => {
    if (status === 'paid') return (
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--success)', fontSize: 14, fontWeight: 700 }}>
        <CheckCircle size={16} /> 支付成功
      </span>
    );
    if (status === 'expired') return (
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--danger)', fontSize: 14, fontWeight: 700 }}>
        <XCircle size={16} /> 已过期
      </span>
    );
    return (
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--warning)', fontSize: 14, fontWeight: 700 }}>
        <Clock size={16} /> 待支付
      </span>
    );
  };

  return (
    <div className="fade-in" style={{ maxWidth: 760, margin: '0 auto' }}>

      {/* 标题区 */}
      <div className="page-header" style={{ marginBottom: 28 }}>
        <div>
          <h1 className="page-title">升级专业版</h1>
          <p className="page-subtitle">
            {user?.plan === 'pro'
              ? <span style={{ color: 'var(--success)', fontWeight: 600 }}>当前已是专业版会员</span>
              : <span>解锁无限应用、无限卡密、无限制设备绑定</span>
            }
          </p>
        </div>
      </div>

      {/* 当前专业版用户不显示购买区 */}
      {user?.plan === 'pro' ? (
        <div style={{
          textAlign: 'center', padding: '60px 0',
          background: 'var(--bg-card)', borderRadius: 16,
          border: '1px solid var(--border)',
        }}>
          <div style={{ fontSize: 48, marginBottom: 16 }}>⚡</div>
          <h2 style={{ fontSize: 20, fontWeight: 800, marginBottom: 8 }}>您已是专业版会员</h2>
          <p style={{ color: 'var(--text-muted)', fontSize: 14 }}>
            感谢您的支持！如需续费，可在下方选择套餐进行充值。
          </p>
        </div>
      ) : !currentOrder ? (
        <>
          {/* 套餐选择 */}
          <div style={{
            background: 'var(--bg-card)', borderRadius: 16,
            border: '1px solid var(--border)', padding: 28, marginBottom: 20,
          }}>
            <h3 style={{ fontSize: 15, fontWeight: 700, marginBottom: 20 }}>选择套餐</h3>
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: 12 }}>
              {PLAN_OPTIONS.map((plan) => (
                <button
                  key={plan.label}
                  onClick={() => setSelectedPlan(plan)}
                  style={{
                    padding: '16px 14px', borderRadius: 12,
                    border: selectedPlan?.label === plan.label
                      ? '2px solid var(--accent)'
                      : '2px solid var(--border)',
                    background: selectedPlan?.label === plan.label
                      ? 'rgba(124,106,247,0.08)'
                      : 'var(--bg)',
                    cursor: 'pointer', textAlign: 'left',
                    transition: 'all 0.15s', position: 'relative',
                  }}
                >
                  {plan.badge && (
                    <span style={{
                      position: 'absolute', top: -10, right: 10,
                      background: plan.highlight ? 'linear-gradient(135deg,#f59e0b,#d97706)' : 'var(--accent)',
                      color: '#fff', fontSize: 10, fontWeight: 700,
                      padding: '2px 7px', borderRadius: 20,
                    }}>
                      {plan.badge}
                    </span>
                  )}
                  <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--text)', marginBottom: 6 }}>
                    {plan.label}
                  </div>
                  <div style={{ fontSize: 22, fontWeight: 800, color: plan.highlight ? 'var(--accent)' : 'var(--text)' }}>
                    ¥{plan.price}
                  </div>
                  {plan.highlight && (
                    <div style={{ fontSize: 11, color: 'var(--accent)', fontWeight: 600, marginTop: 4 }}>
                      推荐 · 无使用期限限制
                    </div>
                  )}
                </button>
              ))}
            </div>
          </div>

          {/* 支付方式 */}
          <div style={{
            background: 'var(--bg-card)', borderRadius: 16,
            border: '1px solid var(--border)', padding: 28, marginBottom: 20,
          }}>
            <h3 style={{ fontSize: 15, fontWeight: 700, marginBottom: 20 }}>选择支付方式</h3>
            <div style={{ display: 'flex', gap: 12 }}>
              {[
                { value: 'wechat', label: '微信支付', icon: '💬' },
                { value: 'alipay', label: '支付宝', icon: '💙' },
              ].map(pt => (
                <button
                  key={pt.value}
                  onClick={() => setPayType(pt.value as PayType)}
                  style={{
                    flex: 1, padding: '14px 16px', borderRadius: 12,
                    border: payType === pt.value ? '2px solid var(--accent)' : '2px solid var(--border)',
                    background: payType === pt.value ? 'rgba(124,106,247,0.08)' : 'var(--bg)',
                    cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 10,
                    transition: 'all 0.15s',
                  }}
                >
                  <span style={{ fontSize: 22 }}>{pt.icon}</span>
                  <span style={{ fontSize: 14, fontWeight: 600, color: 'var(--text)' }}>{pt.label}</span>
                  {payType === pt.value && (
                    <CheckCircle size={16} style={{ marginLeft: 'auto', color: 'var(--accent)' }} />
                  )}
                </button>
              ))}
            </div>
          </div>

          {/* 购买按钮 */}
          <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
            <button
              className="btn btn-primary"
              style={{ padding: '12px 36px', fontSize: 15 }}
              onClick={handleCreateOrder}
              disabled={creating || !selectedPlan}
            >
              {creating ? (
                <><span className="spinner" /> 生成订单中…</>
              ) : (
                <><CreditCard size={15} /> 立即支付 ¥{selectedPlan?.price ?? '0.00'}</>
              )}
            </button>
          </div>
        </>
      ) : (
        /* 扫码支付区 */
        <div style={{
          background: 'var(--bg-card)', borderRadius: 16,
          border: '1px solid var(--border)', padding: 32, textAlign: 'center',
        }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 24 }}>
            <div>
              <h3 style={{ fontSize: 17, fontWeight: 800, marginBottom: 4 }}>
                {currentOrder.expires_days
                  ? `KamiSM 专业版 ${currentOrder.expires_days} 天`
                  : 'KamiSM 专业版（永久）'}
              </h3>
              <div style={{ fontSize: 26, fontWeight: 900, color: 'var(--accent)' }}>
                ¥{currentOrder.price}
              </div>
            </div>
            <div style={{ textAlign: 'right' }}>
              <div style={{ marginBottom: 6 }}><StatusBadge status={currentOrder.status} /></div>
              {currentOrder.status === 'pending' && (
                <button className="btn btn-ghost" style={{ fontSize: 12 }} onClick={handleCancelOrder}>
                  取消订单
                </button>
              )}
            </div>
          </div>

          {/* 二维码 */}
          {currentOrder.status === 'pending' && (
            <div style={{ marginBottom: 16 }}>
              {currentOrder.qr_url ? (
                <div style={{
                  display: 'inline-block', padding: 16, background: '#fff',
                  borderRadius: 16, border: '1px solid var(--border)',
                }}>
                  <QrCode value={currentOrder.qr_url} size={220} />
                  <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-muted)' }}>
                    {currentOrder.pay_type === 'wechat' ? '请使用微信扫码支付' : '请使用支付宝扫码支付'}
                  </div>
                  <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 4 }}>
                    二维码有效期 {Math.floor(currentOrder.expires_in / 60)} 分钟
                  </div>
                </div>
              ) : (
                <div style={{ padding: '40px 0', color: 'var(--text-muted)' }}>
                  <span className="spinner" /> 正在生成二维码…
                </div>
              )}
            </div>
          )}

          {/* 支付成功 */}
          {currentOrder.status === 'paid' && (
            <div style={{ padding: '32px 0' }}>
              <CheckCircle size={64} style={{ color: 'var(--success)', marginBottom: 16 }} />
              <h2 style={{ fontSize: 20, fontWeight: 800, marginBottom: 8 }}>支付成功！</h2>
              <p style={{ color: 'var(--text-muted)', fontSize: 14 }}>
                专业版已开通，正在跳转…
              </p>
            </div>
          )}

          {/* 已过期 */}
          {currentOrder.status === 'expired' && (
            <div style={{ padding: '32px 0' }}>
              <XCircle size={64} style={{ color: 'var(--danger)', marginBottom: 16 }} />
              <h2 style={{ fontSize: 20, fontWeight: 800, marginBottom: 8 }}>二维码已过期</h2>
              <button className="btn btn-primary" onClick={handleCancelOrder}>
                <RefreshCw size={14} /> 重新发起支付
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// 二维码组件（使用 qrcode npm 包生成）
function QrCode({ value, size }: { value: string; size: number }) {
  const [src, setSrc] = useState('');
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!value) { setSrc(''); return; }
    let cancelled = false;
    setLoading(true);
    import('qrcode').then(({ default: QRCode }) => {
      if (cancelled) return;
      QRCode.toDataURL(value, {
        width: size,
        margin: 2,
        color: { dark: '#000000', light: '#ffffff' },
      }).then((dataUrl: string) => {
        if (!cancelled) { setSrc(dataUrl); setLoading(false); }
      }).catch(() => {
        if (!cancelled) { setSrc(''); setLoading(false); }
      });
    }).catch(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [value, size]);

  if (loading) {
    return <div style={{ width: size, height: size, background: '#f5f5f5', display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: 8 }}><span className="spinner" /></div>;
  }
  if (!src) {
    return <div style={{ width: size, height: size, background: '#f5f5f5', display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: 8, fontSize: 12, color: '#999' }}>加载中…</div>;
  }
  return <img src={src} width={size} height={size} alt="支付二维码" style={{ display: 'block', borderRadius: 8 }} />;
}
