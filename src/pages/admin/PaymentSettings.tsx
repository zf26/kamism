import { useEffect, useState } from 'react';
import { api } from '../../lib/api';
import toast from 'react-hot-toast';

interface PaymentChannel {
  id: string;
  channel: string;
  name: string;
  enabled: boolean;
  alipay_app_id_set?: boolean;
  xorpay_aid_set?: boolean;
  mbdpay_app_id_set?: boolean;
}

interface ChannelConfig {
  id: string;
  channel: string;
  name: string;
  enabled: boolean;
  xorpay_aid?: string;
  xorpay_app_key?: string;
  xorpay_notify_url?: string;
  mbdpay_app_id?: string;
  mbdpay_app_key?: string;
  mbdpay_notify_url?: string;
  alipay_app_id?: string;
  alipay_private_key?: string;
  alipay_public_key?: string;
  alipay_notify_url?: string;
  alipay_gateway?: string;
  alipay_return_url?: string;
}

const channelIcons: Record<string, React.ReactNode> = {
  alipay: (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
      <rect x="2" y="2" width="20" height="20" rx="4" fill="#1677FF"/>
      <text x="12" y="16" textAnchor="middle" fill="#fff" fontSize="10" fontWeight="bold" fontFamily="sans-serif">支</text>
    </svg>
  ),
  xorpay: (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
      <rect x="2" y="2" width="20" height="20" rx="4" fill="#10B981"/>
      <text x="12" y="16" textAnchor="middle" fill="#fff" fontSize="10" fontWeight="bold" fontFamily="sans-serif">XP</text>
    </svg>
  ),
  mbdpay: (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
      <rect x="2" y="2" width="20" height="20" rx="4" fill="#F59E0B"/>
      <text x="12" y="16" textAnchor="middle" fill="#fff" fontSize="10" fontWeight="bold" fontFamily="sans-serif">面</text>
    </svg>
  ),
};

function FieldGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="form-group">
      <label className="form-label">{label}</label>
      {children}
    </div>
  );
}

function AlipayForm({
  cfg,
  form,
  setForm,
  saving,
  onSave,
}: {
  cfg: ChannelConfig;
  form: Record<string, string>;
  setForm: (f: Record<string, string>) => void;
  saving: boolean;
  onSave: () => void;
}) {
  const set = (k: string) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setForm({ ...form, [k]: e.target.value });

  return (
    <div>
      <div style={{ marginBottom: 20, padding: '12px 16px', background: 'var(--bg-hover)', borderRadius: 8, border: '1px solid var(--border)', fontSize: 13, color: 'var(--text-muted)' }}>
        在 <a href="https://open.alipay.com" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>支付宝开放平台</a> 创建应用后，将回调地址填写为：<br />
        <code style={{ color: 'var(--text)', marginTop: 4, display: 'block' }}>{form.alipay_notify_url || cfg.alipay_notify_url}</code>
      </div>

      <FieldGroup label="App ID（应用 ID）">
        <input type="text" value={form.alipay_app_id || ''} onChange={set('alipay_app_id')} placeholder="例如：2021001234567890" style={{ maxWidth: 380 }} />
      </FieldGroup>

      <FieldGroup label="应用私钥（RSA2，PKCS8 格式）">
        <textarea
          value={form.alipay_private_key || ''}
          onChange={set('alipay_private_key')}
          placeholder="请粘贴 RSA2 应用私钥（PKCS8 格式）"
          rows={5}
          style={{ maxWidth: 580, resize: 'vertical', fontFamily: 'monospace', fontSize: 12 }}
        />
        <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
          支持粘贴带或不带 PEM 头尾的私钥内容
        </p>
      </FieldGroup>

      <FieldGroup label="支付宝公钥">
        <textarea
          value={form.alipay_public_key || ''}
          onChange={set('alipay_public_key')}
          placeholder="请粘贴支付宝公钥"
          rows={3}
          style={{ maxWidth: 580, resize: 'vertical', fontFamily: 'monospace', fontSize: 12 }}
        />
      </FieldGroup>

      <FieldGroup label="回调地址（notify_url）">
        <input type="text" value={form.alipay_notify_url || cfg.alipay_notify_url || ''} onChange={set('alipay_notify_url')} placeholder="例如：https://your-domain.com/api/pay/notify" style={{ maxWidth: 500 }} />
        <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>需要在支付宝开放平台配置</p>
      </FieldGroup>

      <FieldGroup label="网关地址">
        <input type="text" value={form.alipay_gateway || cfg.alipay_gateway || ''} onChange={set('alipay_gateway')} placeholder="https://openapi.alipay.com/gateway.do" style={{ maxWidth: 500 }} />
      </FieldGroup>

      <FieldGroup label="支付完成跳转地址（return_url）">
        <input type="text" value={form.alipay_return_url || cfg.alipay_return_url || ''} onChange={set('alipay_return_url')} placeholder="例如：https://your-domain.com/dashboard" style={{ maxWidth: 500 }} />
      </FieldGroup>

      <div style={{ marginTop: 24, paddingTop: 24, borderTop: '1px solid var(--border-light)' }}>
        <button className="btn btn-primary" onClick={onSave} disabled={saving}>
          {saving ? <span className="spinner" /> : '保存配置'}
        </button>
      </div>
    </div>
  );
}

function XorPayForm({
  form,
  setForm,
  saving,
  onSave,
}: {
  form: Record<string, string>;
  setForm: (f: Record<string, string>) => void;
  saving: boolean;
  onSave: () => void;
}) {
  const set = (k: string) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setForm({ ...form, [k]: e.target.value });

  return (
    <div>
      <div style={{ marginBottom: 20, padding: '12px 16px', background: 'var(--bg-hover)', borderRadius: 8, border: '1px solid var(--border)', fontSize: 13, color: 'var(--text-muted)' }}>
        XorPay 官方地址：<a href="https://xorpay.com" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>https://xorpay.com</a>
      </div>

      <FieldGroup label="AID">
        <input type="text" value={form.xorpay_aid || ''} onChange={set('xorpay_aid')} placeholder="请输入 XorPay AID" style={{ maxWidth: 380 }} />
      </FieldGroup>

      <FieldGroup label="App Key">
        <input type="password" value={form.xorpay_app_key || ''} onChange={set('xorpay_app_key')} placeholder="请输入 XorPay App Key" style={{ maxWidth: 380 }} />
      </FieldGroup>

      <FieldGroup label="回调地址（notify_url）">
        <input type="text" value={form.xorpay_notify_url || ''} onChange={set('xorpay_notify_url')} placeholder="例如：https://your-domain.com/api/pay/notify" style={{ maxWidth: 500 }} />
      </FieldGroup>

      <div style={{ marginTop: 24, paddingTop: 24, borderTop: '1px solid var(--border-light)' }}>
        <button className="btn btn-primary" onClick={onSave} disabled={saving}>
          {saving ? <span className="spinner" /> : '保存配置'}
        </button>
      </div>
    </div>
  );
}

function MbdPayForm({
  form,
  setForm,
  saving,
  onSave,
}: {
  form: Record<string, string>;
  setForm: (f: Record<string, string>) => void;
  saving: boolean;
  onSave: () => void;
}) {
  const set = (k: string) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setForm({ ...form, [k]: e.target.value });

  return (
    <div>
      <div style={{ marginBottom: 20, padding: '12px 16px', background: 'var(--bg-hover)', borderRadius: 8, border: '1px solid var(--border)', fontSize: 13, color: 'var(--text-muted)' }}>
        面包多官方地址：<a href="https://mbd.pub" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>https://mbd.pub</a>
      </div>

      <FieldGroup label="App ID">
        <input type="text" value={form.mbdpay_app_id || ''} onChange={set('mbdpay_app_id')} placeholder="请输入面包多 App ID" style={{ maxWidth: 380 }} />
      </FieldGroup>

      <FieldGroup label="App Key">
        <input type="password" value={form.mbdpay_app_key || ''} onChange={set('mbdpay_app_key')} placeholder="请输入面包多 App Key" style={{ maxWidth: 380 }} />
      </FieldGroup>

      <FieldGroup label="回调地址（notify_url）">
        <input type="text" value={form.mbdpay_notify_url || ''} onChange={set('mbdpay_notify_url')} placeholder="例如：https://your-domain.com/api/pay/notify" style={{ maxWidth: 500 }} />
      </FieldGroup>

      <div style={{ marginTop: 24, paddingTop: 24, borderTop: '1px solid var(--border-light)' }}>
        <button className="btn btn-primary" onClick={onSave} disabled={saving}>
          {saving ? <span className="spinner" /> : '保存配置'}
        </button>
      </div>
    </div>
  );
}

export default function PaymentSettings() {
  const [channels, setChannels] = useState<PaymentChannel[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeChannel, setActiveChannel] = useState<string | null>(null);
  const [config, setConfig] = useState<ChannelConfig | null>(null);
  const [configLoading, setConfigLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [form, setForm] = useState<Record<string, string>>({});

  useEffect(() => { loadChannels(); }, []);

  const loadChannels = async () => {
    setLoading(true);
    try {
      const res = await api.get('/admin/payment/configs');
      if (res.data.success) {
        setChannels(res.data.data);
        if (res.data.data.length > 0) {
          loadChannelConfig(res.data.data[0].channel);
        }
      } else {
        toast.error(res.data.message || '加载配置失败');
      }
    } catch {

    } finally {
      setLoading(false);
    }
  };

  const loadChannelConfig = async (channel: string) => {
    try {
      setConfigLoading(true);
      setActiveChannel(channel);
      const res = await api.get(`/admin/payment/configs/${channel}`);
      if (res.data.success) {
        setConfig(res.data.data);
        setForm({
          xorpay_aid: res.data.data.xorpay_aid || '',
          xorpay_app_key: res.data.data.xorpay_app_key || '',
          xorpay_notify_url: res.data.data.xorpay_notify_url || '',
          mbdpay_app_id: res.data.data.mbdpay_app_id || '',
          mbdpay_app_key: res.data.data.mbdpay_app_key || '',
          mbdpay_notify_url: res.data.data.mbdpay_notify_url || '',
          alipay_app_id: res.data.data.alipay_app_id || '',
          alipay_private_key: res.data.data.alipay_private_key || '',
          alipay_public_key: res.data.data.alipay_public_key || '',
          alipay_notify_url: res.data.data.alipay_notify_url || '',
          alipay_gateway: res.data.data.alipay_gateway || '',
          alipay_return_url: res.data.data.alipay_return_url || '',
        });
      }
    } catch {
      toast.error('加载配置失败');
    } finally {
      setConfigLoading(false);
    }
  };

  const handleSave = async () => {
    if (!activeChannel) return;
    setSaving(true);
    try {
      const updateData: Record<string, string> = {};
      if (activeChannel === 'xorpay') {
        if (form.xorpay_aid) updateData.xorpay_aid = form.xorpay_aid;
        if (form.xorpay_app_key) updateData.xorpay_app_key = form.xorpay_app_key;
        if (form.xorpay_notify_url) updateData.xorpay_notify_url = form.xorpay_notify_url;
      } else if (activeChannel === 'mbdpay') {
        if (form.mbdpay_app_id) updateData.mbdpay_app_id = form.mbdpay_app_id;
        if (form.mbdpay_app_key) updateData.mbdpay_app_key = form.mbdpay_app_key;
        if (form.mbdpay_notify_url) updateData.mbdpay_notify_url = form.mbdpay_notify_url;
      } else if (activeChannel === 'alipay') {
        if (form.alipay_app_id) updateData.alipay_app_id = form.alipay_app_id;
        if (form.alipay_private_key) updateData.alipay_private_key = form.alipay_private_key;
        if (form.alipay_public_key) updateData.alipay_public_key = form.alipay_public_key;
        if (form.alipay_notify_url) updateData.alipay_notify_url = form.alipay_notify_url;
        if (form.alipay_gateway) updateData.alipay_gateway = form.alipay_gateway;
        if (form.alipay_return_url) updateData.alipay_return_url = form.alipay_return_url;
      }

      const res = await api.patch(`/admin/payment/configs/${activeChannel}`, updateData);
      if (res.data.success) {
        toast.success('保存成功');
        loadChannels();
      } else {
        toast.error(res.data.message || '保存失败');
      }
    } catch {
      toast.error('保存失败');
    } finally {
      setSaving(false);
    }
  };

  const handleToggle = async (channel: string, enabled: boolean) => {
    try {
      const res = await api.post(`/admin/payment/configs/${channel}/toggle`, { enabled });
      if (res.data.success) {
        toast.success(res.data.message);
        loadChannels();
      } else {
        toast.error(res.data.message || '操作失败');
      }
    } catch {
      toast.error('操作失败');
    }
  };

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-title">支付配置（Beta）</h1>
          <p className="page-desc">配置支付渠道，管理商户收款</p>
        </div>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: '280px 1fr', gap: 24, alignItems: 'start' }}>
        {/* 左侧：渠道列表 */}
        <div className="card" style={{ padding: 0 }}>
          <div style={{ padding: '16px 20px', borderBottom: '1px solid var(--border-light)' }}>
            <span style={{ fontWeight: 600 }}>支付渠道</span>
          </div>
          {loading ? (
            <div style={{ padding: 40, textAlign: 'center' }}><span className="spinner" /></div>
          ) : channels.length === 0 ? (
            <div style={{ padding: 40, textAlign: 'center', color: 'var(--text-muted)', fontSize: 13 }}>
              暂无可用渠道
            </div>
          ) : (
            channels.map(ch => (
              <div
                key={ch.channel}
                onClick={() => loadChannelConfig(ch.channel)}
                style={{
                  padding: '14px 20px',
                  borderBottom: '1px solid var(--border-light)',
                  cursor: 'pointer',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  background: activeChannel === ch.channel ? 'var(--bg-hover)' : 'transparent',
                  transition: 'background 0.15s',
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  {channelIcons[ch.channel] || (
                    <div style={{ width: 22, height: 22, borderRadius: 4, background: 'var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 12 }}>?</div>
                  )}
                  <div>
                    <div style={{ fontWeight: activeChannel === ch.channel ? 600 : 400 }}>{ch.name}</div>
                    {ch.channel === 'alipay' && ch.alipay_app_id_set && (
                      <div style={{ fontSize: 11, color: 'var(--success)' }}>已配置</div>
                    )}
                    {ch.channel === 'xorpay' && ch.xorpay_aid_set && (
                      <div style={{ fontSize: 11, color: 'var(--success)' }}>已配置</div>
                    )}
                    {ch.channel === 'mbdpay' && ch.mbdpay_app_id_set && (
                      <div style={{ fontSize: 11, color: 'var(--success)' }}>已配置</div>
                    )}
                  </div>
                </div>
                <label className="toggle" onClick={e => e.stopPropagation()}>
                  <input
                    type="checkbox"
                    checked={ch.enabled}
                    onChange={e => handleToggle(ch.channel, e.target.checked)}
                  />
                  <span className="toggle-slider" />
                </label>
              </div>
            ))
          )}
        </div>

        {/* 右侧：配置表单 */}
        <div className="card">
          {!activeChannel ? (
            <div style={{ padding: 60, textAlign: 'center', color: 'var(--text-muted)' }}>
              请选择一个支付渠道进行配置
            </div>
          ) : configLoading ? (
            <div style={{ padding: 60, textAlign: 'center' }}><span className="spinner" /></div>
          ) : config && (
            <div>
              <div style={{ marginBottom: 24 }}>
                <h3 style={{ fontSize: 16, fontWeight: 600, marginBottom: 4 }}>
                  {config.name}
                </h3>
                <p style={{ fontSize: 13, color: 'var(--text-muted)' }}>
                  填写从 {config.name} 平台获取的凭据信息
                </p>
              </div>

              {activeChannel === 'alipay' && (
                <AlipayForm
                  cfg={config}
                  form={form}
                  setForm={setForm}
                  saving={saving}
                  onSave={handleSave}
                />
              )}
              {activeChannel === 'xorpay' && (
                <XorPayForm
                  form={form}
                  setForm={setForm}
                  saving={saving}
                  onSave={handleSave}
                />
              )}
              {activeChannel === 'mbdpay' && (
                <MbdPayForm
                  form={form}
                  setForm={setForm}
                  saving={saving}
                  onSave={handleSave}
                />
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
