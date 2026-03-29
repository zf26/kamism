import { useState } from 'react';
import { useAuthStore } from '../../stores/auth';
import { Copy, Check } from 'lucide-react';
import toast from 'react-hot-toast';

const BASE_URL = import.meta.env.VITE_API_URL || 'http://localhost:9527';

type Tab = 'activate' | 'verify' | 'unbind';

interface Endpoint {
  id: Tab;
  label: string;
  method: 'POST';
  path: string;
  desc: string;
  headers: { key: string; value: string; desc: string }[];
  body: Record<string, string>;
  response: object;
  note?: string;
}

function buildEndpoints(apiKey: string): Endpoint[] {
  return [
    {
      id: 'activate',
      label: '激活',
      method: 'POST',
      path: '/v1/activate',
      desc: '用卡密激活软件并绑定当前设备。首次激活时自动完成绑定；若该设备已绑定此卡密则直接返回成功。',
      headers: [
        { key: 'Content-Type', value: 'application/json', desc: '固定值' },
      ],
      body: {
        api_key: apiKey,
        app_id: '<app_id>',
        card_code: '<card_code>',
        device_id: '<device_id>',
        device_name: '<optional>',
      },
      response: {
        success: true,
        message: '激活成功',
        data: {
          expires_at: '2026-12-31T00:00:00Z',
          remaining_days: 276,
          max_devices: 3,
          current_devices: 1,
        },
      },
      note: 'device_id 建议使用设备唯一标识（如主板序列号、MAC 地址等），长度不超过 128 字符。device_name 为可选字段。',
    },
    {
      id: 'verify',
      label: '验证',
      method: 'POST',
      path: '/v1/verify',
      desc: '每次软件启动时调用，校验卡密是否有效且设备已绑定。建议每次启动必验，防离线破解。服务端有 60s Redis 缓存，高频调用无额外性能损耗。',
      headers: [
        { key: 'Content-Type', value: 'application/json', desc: '固定值' },
      ],
      body: {
        api_key: apiKey,
        app_id: '<app_id>',
        card_code: '<card_code>',
        device_id: '<device_id>',
      },
      response: {
        success: true,
        valid: true,
        message: '卡密有效',
        data: {
          expires_at: '2026-12-31T00:00:00Z',
          remaining_days: 276,
          max_devices: 3,
          current_devices: 1,
        },
      },
      note: '返回 success: false 或 valid: false 时软件应拒绝运行，并展示 message 内容给用户。',
    },
    {
      id: 'unbind',
      label: '解绑',
      method: 'POST',
      path: '/v1/unbind',
      desc: '解除指定设备与卡密的绑定关系，释放设备配额。解绑后该设备需重新激活才可使用。',
      headers: [
        { key: 'Content-Type', value: 'application/json', desc: '固定值' },
      ],
      body: {
        api_key: apiKey,
        app_id: '<app_id>',
        card_code: '<card_code>',
        device_id: '<device_id>',
      },
      response: {
        success: true,
        message: '设备已解绑',
      },
      note: '商户也可以在「激活记录」页面手动解绑设备，无需调用此接口。若解绑后该卡密无绑定设备，卡密状态将自动重置为 unused。',
    },
  ];
}

function CodeBlock({ code, lang = 'json' }: { code: string; lang?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard.writeText(code);
    setCopied(true);
    toast.success('已复制');
    setTimeout(() => setCopied(false), 2000);
  };
  return (
    <div style={{ position: 'relative', marginBottom: 0 }}>
      <div style={{
        position: 'absolute', top: 10, right: 10, zIndex: 1,
        display: 'flex', alignItems: 'center', gap: 4,
      }}>
        <span style={{ fontSize: 10, color: 'var(--text-muted)', fontFamily: 'var(--mono)', textTransform: 'uppercase' }}>{lang}</span>
        <button
          onClick={copy}
          style={{
            background: 'var(--bg-hover)', border: '1px solid var(--border)',
            borderRadius: 5, padding: '3px 8px', cursor: 'pointer',
            color: 'var(--text-muted)', display: 'flex', alignItems: 'center', gap: 4, fontSize: 11,
          }}
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
          {copied ? '已复制' : '复制'}
        </button>
      </div>
      <pre className="mono" style={{
        background: 'var(--bg)',
        border: '1px solid var(--border)',
        borderRadius: 8,
        padding: '14px 16px',
        paddingRight: 80,
        fontSize: 12,
        lineHeight: 1.7,
        overflowX: 'auto',
        color: 'var(--text-dim)',
        margin: 0,
        whiteSpace: 'pre-wrap',
        wordBreak: 'break-all',
      }}>
        {code}
      </pre>
    </div>
  );
}

export default function ApiDocs() {
  const { user } = useAuthStore();
  const [activeTab, setActiveTab] = useState<Tab>('activate');

  const apiKey = user?.api_key ?? '<your_api_key>';
  const endpoints = buildEndpoints(apiKey);
  const ep = endpoints.find((e) => e.id === activeTab)!;

  const curl =
    `curl -X POST "${BASE_URL}${ep.path}" \\\n` +
    `  -H "Content-Type: application/json" \\\n` +
    `  -d '${JSON.stringify(ep.body)}'`;

  const pythonCode =
    `import requests\n\n` +
    `url = "${BASE_URL}${ep.path}"\n` +
    `payload = ${JSON.stringify(ep.body, null, 4)}\n\n` +
    `response = requests.post(url, json=payload)\n` +
    `print(response.json())`;

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">API 接口文档</h1>
          <p className="page-subtitle">集成激活、验证、解绑接口到你的软件</p>
        </div>
      </div>

      {/* Base URL + API Key 提示 */}
      <div style={{
        background: 'rgba(124,106,247,0.07)',
        border: '1px solid rgba(124,106,247,0.2)',
        borderRadius: 10,
        padding: '12px 16px',
        marginBottom: 24,
        display: 'flex',
        alignItems: 'center',
        gap: 12,
        flexWrap: 'wrap',
      }}>
        <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>Base URL</span>
        <code className="mono" style={{ fontSize: 12, color: 'var(--accent)', flex: 1 }}>{BASE_URL}</code>
        <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>你的 API Key</span>
        <code className="mono" style={{
          fontSize: 12, color: 'var(--accent)',
          background: 'var(--bg)', border: '1px solid var(--border)',
          borderRadius: 5, padding: '2px 8px',
        }}>
          {user?.api_key ?? '请在账号设置中查看'}
        </code>
      </div>

      {/* Tabs */}
      <div style={{ display: 'flex', gap: 6, marginBottom: 20 }}>
        {endpoints.map((e) => (
          <button
            key={e.id}
            onClick={() => setActiveTab(e.id)}
            style={{
              padding: '7px 18px',
              borderRadius: 8,
              fontSize: 13,
              fontWeight: activeTab === e.id ? 700 : 500,
              border: activeTab === e.id ? '1px solid rgba(124,106,247,0.3)' : '1px solid var(--border)',
              background: activeTab === e.id ? 'var(--accent-glow)' : 'var(--bg-card)',
              color: activeTab === e.id ? 'var(--accent)' : 'var(--text-dim)',
              cursor: 'pointer',
              transition: 'all 0.15s',
            }}
          >
            {e.label}
          </button>
        ))}
      </div>

      <div style={{ display: 'grid', gap: 16 }}>
        {/* 接口概览 */}
        <div className="card">
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
            <span style={{
              background: '#10b98122',
              color: '#10b981',
              fontFamily: 'var(--mono)',
              fontSize: 11,
              fontWeight: 800,
              padding: '3px 8px',
              borderRadius: 5,
              letterSpacing: '0.5px',
            }}>POST</span>
            <code className="mono" style={{ fontSize: 13, color: 'var(--text)' }}>{BASE_URL}{ep.path}</code>
          </div>
          <p style={{ fontSize: 13, color: 'var(--text-dim)', lineHeight: 1.7, margin: 0 }}>{ep.desc}</p>
          {ep.note && (
            <div style={{
              marginTop: 12,
              padding: '8px 12px',
              background: 'rgba(245,158,11,0.08)',
              border: '1px solid rgba(245,158,11,0.2)',
              borderRadius: 6,
              fontSize: 12,
              color: '#d97706',
              lineHeight: 1.6,
            }}>
              {ep.note}
            </div>
          )}
        </div>

        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 16 }}>
          {/* 请求头 */}
          <div className="card">
            <p style={{ fontWeight: 700, color: 'var(--text)', marginBottom: 12, fontSize: 13 }}>请求头</p>
            <div style={{ display: 'grid', gap: 8 }}>
              {ep.headers.map((h) => (
                <div key={h.key} style={{
                  display: 'grid', gridTemplateColumns: 'auto 1fr',
                  gap: '4px 12px', alignItems: 'start',
                  padding: '8px 0', borderBottom: '1px solid var(--border)',
                }}>
                  <code className="mono" style={{ fontSize: 11, color: 'var(--accent)', whiteSpace: 'nowrap' }}>{h.key}</code>
                  <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>{h.desc}</span>
                  <span />
                  <code className="mono" style={{ fontSize: 11, color: 'var(--text-dim)' }}>{h.value}</code>
                </div>
              ))}
            </div>
          </div>

          {/* 请求体 */}
          <div className="card">
            <p style={{ fontWeight: 700, color: 'var(--text)', marginBottom: 12, fontSize: 13 }}>请求体 (JSON)</p>
            <CodeBlock code={JSON.stringify(ep.body, null, 2)} />
          </div>
        </div>

        {/* 响应示例 */}
        <div className="card">
          <p style={{ fontWeight: 700, color: 'var(--text)', marginBottom: 12, fontSize: 13 }}>响应示例</p>
          <CodeBlock code={JSON.stringify(ep.response, null, 2)} />
        </div>

        {/* 代码示例 */}
        <div className="card">
          <p style={{ fontWeight: 700, color: 'var(--text)', marginBottom: 16, fontSize: 13 }}>代码示例</p>
          <div style={{ display: 'grid', gap: 12 }}>
            <div>
              <p style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 6 }}>cURL</p>
              <CodeBlock code={curl} lang="bash" />
            </div>
            <div>
              <p style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 6 }}>Python</p>
              <CodeBlock code={pythonCode} lang="python" />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
