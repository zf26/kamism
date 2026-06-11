import { useEffect, useState } from 'react';
import { api } from '../../lib/api';
import toast from 'react-hot-toast';

interface OAuthProvider {
  id: string;
  provider: string;
  name: string;
  enabled: boolean;
  scopes: string;
}

interface OAuthConfig {
  id: string;
  provider: string;
  name: string;
  client_id: string;
  client_secret_set: boolean;
  auth_url: string;
  token_url: string;
  userinfo_url: string;
  scopes: string;
  enabled: boolean;
}

interface CreateForm {
  provider: string;
  name: string;
  auth_url: string;
  token_url: string;
  userinfo_url: string;
  scopes: string;
}

const providerIcons: Record<string, React.ReactNode> = {
  github: (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor">
      <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
    </svg>
  ),
  google: (
    <svg width="20" height="20" viewBox="0 0 24 24">
      <path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"/>
      <path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/>
      <path fill="#FBBC05" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/>
      <path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/>
    </svg>
  ),
  microsoft: (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none">
      <rect x="1" y="1" width="10" height="10" fill="#F25022"/>
      <rect x="13" y="1" width="10" height="10" fill="#7FBA00"/>
      <rect x="1" y="13" width="10" height="10" fill="#00A4EF"/>
      <rect x="13" y="13" width="10" height="10" fill="#FFB900"/>
    </svg>
  ),
  qq: (
    <svg width="18" height="18" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M6.048 3.323c.022.277-.13.523-.338.55-.21.026-.397-.176-.419-.453s.13-.523.338-.55c.21-.026.397.176.42.453Zm2.265-.24c-.603-.146-.894.256-.936.333-.027.048-.008.117.037.15.045.035.092.025.119-.003.361-.39.751-.172.829-.129l.011.007c.053.024.147.028.193-.098.023-.063.017-.11-.006-.142-.016-.023-.089-.08-.247-.118" fill="#12B7F5"/>
      <path d="M11.727 6.719c0-.022.01-.375.01-.557 0-3.07-1.45-6.156-5.015-6.156S1.708 3.092 1.708 6.162c0 .182.01.535.01.557l-.72 1.795a26 26 0 0 0-.534 1.508c-.68 2.187-.46 3.093-.292 3.113.36.044 1.401-1.647 1.401-1.647 0 .979.504 2.256 1.594 3.179-.408.126-.907.319-1.228.556-.29.213-.253.43-.201.518.228.386 3.92.246 4.985.126 1.065.12 4.756.26 4.984-.126.052-.088.088-.305-.2-.518-.322-.237-.822-.43-1.23-.557 1.09-.922 1.594-2.2 1.594-3.178 0 0 1.041 1.69 1.401 1.647.168-.02.388-.926-.292-3.113a26 26 0 0 0-.534-1.508l-.72-1.795ZM9.773 5.53a.1.1 0 0 1-.009.096c-.109.159-1.554.943-3.033.943h-.017c-1.48 0-2.925-.784-3.034-.943a.1.1 0 0 1-.018-.055q0-.022.01-.04c.13-.287 1.43-.606 3.042-.606h.017c1.611 0 2.912.319 3.042.605m-4.32-.989c-.483.022-.896-.529-.922-1.229s.344-1.286.828-1.308c.483-.022.896.529.922 1.23.027.7-.344 1.286-.827 1.307Zm2.538 0c-.484-.022-.854-.607-.828-1.308.027-.7.44-1.25.923-1.23.483.023.853.608.827 1.309-.026.7-.439 1.251-.922 1.23ZM2.928 8.99q.32.063.639.117v2.336s1.104.222 2.21.068V9.363q.49.027.937.023h.017c1.117.013 2.474-.136 3.786-.396.097.622.151 1.386.097 2.284-.146 2.45-1.6 3.99-3.846 4.012h-.091c-2.245-.023-3.7-1.562-3.846-4.011-.054-.9 0-1.663.097-2.285" fill="#12B7F5"/>
    </svg>
  ),
  wechat: (
    <svg width="18" height="18" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M11.176 14.429c-2.665 0-4.826-1.8-4.826-4.018 0-2.22 2.159-4.02 4.824-4.02S16 8.191 16 10.411c0 1.21-.65 2.301-1.666 3.036a.32.32 0 0 0-.12.366l.218.81a.6.6 0 0 1.029.117.166.166 0 0 1-.162.162.2.2 0 0 1-.092-.03l-1.057-.61a.5.5 0 0 0-.256-.074.5.5 0 0 0-.142.021 5.7 5.7 0 0 1-1.576.22M9.064 9.542a.647.647 0 1 0 .557-1 .645.645 0 0 0-.646.647.6.6 0 0 0 .09.353Zm3.232.001a.646.646 0 1 0 .546-1 .645.645 0 0 0-.644.644.63.63 0 0 0 .098.356" fill="#07C160"/>
      <path d="M0 6.826c0 1.455.781 2.765 2.001 3.656a.385.385 0 0 1.143.439l-.161.6-.1.373a.5.5 0 0 0-.032.14.19.19 0 0 0 .193.193q.06 0 .111-.029l1.268-.733a.6.6 0 0 1.308-.088q.088 0 .171.025a6.8 6.8 0 0 0 1.625.26 4.5 4.5 0 0 1-.177-1.251c0-2.936 2.785-5.02 5.824-5.02l.15.002C10.587 3.429 8.392 2 5.796 2 2.596 2 0 4.16 0 6.826m4.632-1.555a.77.77 0 1 1-1.54 0 .77.77 0 0 1 1.54 0m3.875 0a.77.77 0 1 1-1.54 0 .77.77 0 0 1 1.54 0" fill="#07C160"/>
    </svg>
  ),
};

function ScopesHint() {
  const [open, setOpen] = useState(false);
  return (
    <div style={{ position: 'relative', display: 'inline-block' }}>
      <button
        type="button"
        onClick={() => setOpen(v => !v)}
        style={{
          background: 'none', border: 'none', cursor: 'pointer',
          color: 'var(--accent)', fontSize: 12, padding: '0 2px',
          verticalAlign: 'middle', marginLeft: 4,
        }}
        title="什么是 Scope？"
      >
        ?
      </button>
      {open && (
        <div
          style={{
            position: 'absolute', top: '100%', left: 0, zIndex: 100,
            width: 320, background: 'var(--bg-card)',
            border: '1px solid var(--border)',
            borderRadius: 8, padding: 14,
            boxShadow: '0 8px 24px rgba(0,0,0,0.2)',
            fontSize: 13, lineHeight: 1.7, color: 'var(--text-dim)',
          }}
          onClick={() => setOpen(false)}
        >
          <div style={{ fontWeight: 700, color: 'var(--text)', marginBottom: 6 }}>Scope（权限范围）是什么？</div>
          Scope 决定 OAuth 登录时你能从第三方平台获取哪些用户信息。不同平台有不同的 scope 名称。
          <div style={{ marginTop: 10, fontWeight: 600, color: 'var(--text)', fontSize: 12 }}>常见示例：</div>
          <div style={{ marginTop: 4 }}>
            <div><b>GitHub</b>: <code>user:email read:user</code></div>
            <div><b>Google</b>: <code>email profile</code></div>
            <div><b>Microsoft</b>: <code>openid email profile</code></div>
          </div>
          <div style={{ marginTop: 8, color: 'var(--text-muted)', fontSize: 12 }}>
            多个 scope 用空格分隔。具体可参考各平台开发者文档。
          </div>
        </div>
      )}
    </div>
  );
}

function AddProviderModal({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const [form, setForm] = useState<CreateForm>({
    provider: '', name: '', auth_url: '', token_url: '', userinfo_url: '', scopes: '',
  });
  const [saving, setSaving] = useState(false);
  const [step, setStep] = useState(1);

  const set = (k: keyof CreateForm) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setForm(f => ({ ...f, [k]: e.target.value }));

  const handleSubmit = async () => {
    if (!form.provider || !form.name || !form.auth_url || !form.token_url || !form.userinfo_url) {
      toast.error('请填写所有必填项');
      return;
    }
    setSaving(true);
    try {
      const res = await api.post('/admin/oauth/providers', {
        provider: form.provider.trim(),
        name: form.name.trim(),
        auth_url: form.auth_url.trim(),
        token_url: form.token_url.trim(),
        userinfo_url: form.userinfo_url.trim(),
        scopes: form.scopes.trim(),
      });
      if (res.data.success) {
        toast.success('添加成功');
        onAdded();
        onClose();
      } else {
        toast.error(res.data.message || '添加失败');
      }
    } catch {
      toast.error('添加失败');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" style={{ maxWidth: 560, width: '90vw' }} onClick={e => e.stopPropagation()}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 20 }}>
          <div style={{ fontSize: 15, fontWeight: 700 }}>添加登录方式</div>
          <button onClick={onClose} style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--text-muted)' }}>
            ✕
          </button>
        </div>

        {/* Step tabs */}
        <div style={{ display: 'flex', gap: 0, marginBottom: 20, borderBottom: '1px solid var(--border)' }}>
          {[['基本信息', 1], ['接口配置', 2]].map(([label, n]) => (
            <button
              key={n}
              onClick={() => setStep(n as number)}
              style={{
                background: 'none', border: 'none', cursor: 'pointer',
                padding: '8px 16px', fontSize: 13, fontWeight: step === n ? 700 : 400,
                color: step === n ? 'var(--accent)' : 'var(--text-muted)',
                borderBottom: step === n ? '2px solid var(--accent)' : '2px solid transparent',
                marginBottom: -1,
              }}
            >
              {label}
            </button>
          ))}
        </div>

        {step === 1 && (
          <div>
            <div className="form-group">
              <label className="form-label">Provider ID <span style={{ color: 'var(--danger)' }}>*</span></label>
              <input
                type="text"
                value={form.provider}
                onChange={set('provider')}
                placeholder="例如: github / bilibili / myapp"
                style={{ maxWidth: 300 }}
              />
              <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                唯一标识符，只能用字母、数字、连字符。不可修改。
              </p>
            </div>
            <div className="form-group">
              <label className="form-label">显示名称 <span style={{ color: 'var(--danger)' }}>*</span></label>
              <input
                type="text"
                value={form.name}
                onChange={set('name')}
                placeholder="例如: GitHub / 哔哩哔哩"
                style={{ maxWidth: 300 }}
              />
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 16 }}>
              <button className="btn btn-primary" onClick={() => setStep(2)}>
                下一步 →
              </button>
            </div>
          </div>
        )}

        {step === 2 && (
          <div>
            <div className="form-group">
              <label className="form-label">授权地址 (Authorization URL) <span style={{ color: 'var(--danger)' }}>*</span></label>
              <input
                type="url"
                value={form.auth_url}
                onChange={set('auth_url')}
                placeholder="https://.../authorize?client_id=...&redirect_uri=...&scope=..."
                style={{ maxWidth: 500 }}
              />
              <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                发起 OAuth 登录的地址，需要包含 <code>client_id</code> <code>redirect_uri</code> <code>scope</code> 参数占位符（用 <code>{"{}"}</code> 或实际值均可）
              </p>
            </div>
            <div className="form-group">
              <label className="form-label">Token 地址 (Token URL) <span style={{ color: 'var(--danger)' }}>*</span></label>
              <input
                type="url"
                value={form.token_url}
                onChange={set('token_url')}
                placeholder="https://.../token"
                style={{ maxWidth: 500 }}
              />
            </div>
            <div className="form-group">
              <label className="form-label">用户信息接口 (Userinfo URL) <span style={{ color: 'var(--danger)' }}>*</span></label>
              <input
                type="url"
                value={form.userinfo_url}
                onChange={set('userinfo_url')}
                placeholder="https://.../userinfo"
                style={{ maxWidth: 500 }}
              />
              <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                获取用户信息（用户名、邮箱等）的接口地址
              </p>
            </div>
            <div className="form-group">
              <label className="form-label" style={{ display: 'flex', alignItems: 'center' }}>
                权限范围 (Scopes) <ScopesHint />
              </label>
              <input
                type="text"
                value={form.scopes}
                onChange={set('scopes')}
                placeholder="多个用空格分隔，例如: email profile read:user"
                style={{ maxWidth: 400 }}
              />
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 16 }}>
              <button className="btn btn-ghost" onClick={() => setStep(1)}>← 上一步</button>
              <button className="btn btn-primary" onClick={handleSubmit} disabled={saving}>
                {saving ? <span className="spinner" /> : '创建'}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default function OAuthSettings() {
  const [providers, setProviders] = useState<OAuthProvider[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [activeProvider, setActiveProvider] = useState<string | null>(null);
  const [config, setConfig] = useState<OAuthConfig | null>(null);
  const [configLoading, setConfigLoading] = useState(false);
  const [showAddModal, setShowAddModal] = useState(false);

  // 详情表单（支持编辑 URL）
  const [form, setForm] = useState({
    client_id: '',
    client_secret: '',
    auth_url: '',
    token_url: '',
    userinfo_url: '',
    scopes: '',
  });

  useEffect(() => { loadProviders(); }, []);

  const loadProviders = async () => {
    setLoading(true);
    try {
      const res = await api.get('/admin/oauth/configs');
      if (res.data.success) {
        setProviders(res.data.data);
        if (res.data.data.length > 0) {
          loadProviderConfig(res.data.data[0].provider);
        }
      } else {
        toast.error(res.data.message || '加载配置失败');
      }
    } catch {

    } finally {
      setLoading(false);
    }
  };

  const loadProviderConfig = async (provider: string) => {
    try {
      setConfigLoading(true);
      setActiveProvider(provider);
      const res = await api.get(`/admin/oauth/configs/${provider}`);
      if (res.data.success) {
        setConfig(res.data.data);
        setForm({
          client_id: res.data.data.client_id || '',
          client_secret: '',
          auth_url: res.data.data.auth_url || '',
          token_url: res.data.data.token_url || '',
          userinfo_url: res.data.data.userinfo_url || '',
          scopes: res.data.data.scopes || '',
        });
      }
    } catch {
      toast.error('加载配置失败');
    } finally {
      setConfigLoading(false);
    }
  };

  const handleSave = async () => {
    if (!activeProvider) return;
    setSaving(activeProvider);
    try {
      const updateData: Record<string, string> = {};
      if (form.client_id) updateData.client_id = form.client_id;
      if (form.client_secret) updateData.client_secret = form.client_secret;
      if (form.auth_url) updateData.auth_url = form.auth_url;
      if (form.token_url) updateData.token_url = form.token_url;
      if (form.userinfo_url) updateData.userinfo_url = form.userinfo_url;
      if (form.scopes) updateData.scopes = form.scopes;

      const res = await api.patch(`/admin/oauth/configs/${activeProvider}`, updateData);
      if (res.data.success) {
        toast.success('保存成功');
        loadProviders();
      } else {
        toast.error(res.data.message || '保存失败');
      }
    } catch {
      toast.error('保存失败');
    } finally {
      setSaving(null);
    }
  };

  const handleToggle = async (provider: string, enabled: boolean) => {
    try {
      const res = await api.post(`/admin/oauth/configs/${provider}/toggle`, { enabled });
      if (res.data.success) {
        toast.success(res.data.message);
        loadProviders();
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
          <h1 className="page-title">OAuth2 配置</h1>
          <p className="page-desc">配置第三方登录，支持任意兼容 OAuth2.0 的平台，开启后在登录页显示</p>
        </div>
        <button className="btn btn-primary" onClick={() => setShowAddModal(true)}>
          + 添加登录方式
        </button>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: '280px 1fr', gap: 24, alignItems: 'start' }}>
        {/* 左侧：平台列表 */}
        <div className="card" style={{ padding: 0 }}>
          <div style={{ padding: '16px 20px', borderBottom: '1px solid var(--border-light)' }}>
            <span style={{ fontWeight: 600 }}>登录平台</span>
          </div>
          {loading ? (
            <div style={{ padding: 40, textAlign: 'center' }}>
              <span className="spinner" />
            </div>
          ) : providers.length === 0 ? (
            <div style={{ padding: 40, textAlign: 'center', color: 'var(--text-muted)', fontSize: 13 }}>
              暂无登录方式<br />点击右上角添加
            </div>
          ) : (
            providers.map(p => (
              <div
                key={p.provider}
                onClick={() => loadProviderConfig(p.provider)}
                style={{
                  padding: '14px 20px',
                  borderBottom: '1px solid var(--border-light)',
                  cursor: 'pointer',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  background: activeProvider === p.provider ? 'var(--bg-hover)' : 'transparent',
                  transition: 'background 0.15s',
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  {providerIcons[p.provider] || (
                    <div style={{ width: 20, height: 20, borderRadius: 4, background: 'var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 12 }}>?</div>
                  )}
                  <span style={{ fontWeight: activeProvider === p.provider ? 600 : 400 }}>{p.name}</span>
                </div>
                <label className="toggle" onClick={e => e.stopPropagation()}>
                  <input
                    type="checkbox"
                    checked={p.enabled}
                    onChange={e => handleToggle(p.provider, e.target.checked)}
                  />
                  <span className="toggle-slider" />
                </label>
              </div>
            ))
          )}
        </div>

        {/* 右侧：配置表单 */}
        <div className="card">
          {!activeProvider ? (
            <div style={{ padding: 60, textAlign: 'center', color: 'var(--text-muted)' }}>
              请选择一个平台进行配置
            </div>
          ) : configLoading ? (
            <div style={{ padding: 60, textAlign: 'center' }}>
              <span className="spinner" />
            </div>
          ) : config && (
            <div>
              <div style={{ marginBottom: 24 }}>
                <h3 style={{ fontSize: 16, fontWeight: 600, marginBottom: 4 }}>
                  {config.name} OAuth 配置
                </h3>
                <p style={{ fontSize: 13, color: 'var(--text-muted)' }}>
                  在下方填写从 {config.name} 开发者平台获取的凭据信息
                </p>
              </div>

              <div className="form-group">
                <label className="form-label">Client ID</label>
                <input
                  type="text"
                  value={form.client_id}
                  onChange={e => setForm(f => ({ ...f, client_id: e.target.value }))}
                  placeholder="请输入 Client ID"
                  style={{ maxWidth: 400 }}
                />
              </div>

              <div className="form-group">
                <label className="form-label">Client Secret</label>
                <input
                  type="password"
                  value={form.client_secret}
                  onChange={e => setForm(f => ({ ...f, client_secret: e.target.value }))}
                  placeholder={config.client_secret_set ? '已设置（留空则不修改）' : '请输入 Client Secret'}
                  style={{ maxWidth: 400 }}
                />
                {config.client_secret_set && (
                  <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>已设置过 Client Secret，留空则保持不变</p>
                )}
              </div>

              <div className="form-group">
                <label className="form-label">回调地址</label>
                <input
                  type="text"
                  value={import.meta.env.VITE_API_URL + '/oauth/' + config.provider + '/callback'}
                  readOnly
                  style={{ maxWidth: 500, opacity: 0.6 }}
                />
                <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                  请将此地址填写到 {config.name} 开发者平台的回调地址设置中
                </p>
              </div>

              <div className="form-group">
                <label className="form-label" style={{ display: 'flex', alignItems: 'center' }}>
                  权限范围 (Scopes) <ScopesHint />
                </label>
                <input
                  type="text"
                  value={form.scopes}
                  onChange={e => setForm(f => ({ ...f, scopes: e.target.value }))}
                  placeholder="多个用空格分隔，例如: email profile read:user"
                  style={{ maxWidth: 400 }}
                />
              </div>

              <div className="form-group">
                <label className="form-label">授权地址 (Authorization URL)</label>
                <input
                  type="text"
                  value={form.auth_url}
                  onChange={e => setForm(f => ({ ...f, auth_url: e.target.value }))}
                  placeholder="https://.../authorize?..."
                  style={{ maxWidth: 500 }}
                />
              </div>

              <div className="form-group">
                <label className="form-label">Token 地址 (Token URL)</label>
                <input
                  type="text"
                  value={form.token_url}
                  onChange={e => setForm(f => ({ ...f, token_url: e.target.value }))}
                  placeholder="https://.../token"
                  style={{ maxWidth: 500 }}
                />
              </div>

              <div className="form-group">
                <label className="form-label">用户信息接口 (Userinfo URL)</label>
                <input
                  type="text"
                  value={form.userinfo_url}
                  onChange={e => setForm(f => ({ ...f, userinfo_url: e.target.value }))}
                  placeholder="https://.../userinfo"
                  style={{ maxWidth: 500 }}
                />
              </div>

              <div style={{ marginTop: 24, paddingTop: 24, borderTop: '1px solid var(--border-light)' }}>
                <button
                  className="btn btn-primary"
                  onClick={handleSave}
                  disabled={saving !== null}
                >
                  {saving === activeProvider ? <span className="spinner" /> : '保存配置'}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      {showAddModal && (
        <AddProviderModal
          onClose={() => setShowAddModal(false)}
          onAdded={() => { loadProviders(); }}
        />
      )}
    </div>
  );
}
