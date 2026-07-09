import { useEffect, useState } from 'react';
import { adminApi, healthApi } from '../../lib/api';
import { Users, Key, Activity, Package, TrendingUp, Database, Server, GitBranch, BarChart3 } from 'lucide-react';
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from 'recharts';

interface Stats {
  merchants: number;
  total_cards: number;
  active_cards: number;
  total_activations: number;
  total_apps: number;
}

interface TrendPoint {
  date: string;
  count: number;
}

interface TrendData {
  merchants: TrendPoint[];
  apps: TrendPoint[];
  cards: TrendPoint[];
}

interface HealthStatus {
  status: 'ok' | 'degraded';
  db: 'ok' | 'error';
  redis: 'ok' | 'error';
  mq: 'ok' | 'error';
  uptime_secs: number;
}

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs} 秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分钟`;
  return `${Math.floor(secs / 86400)} 天 ${Math.floor((secs % 86400) / 3600)} 小时`;
}

const chartColors = {
  merchants: '#7c6af7',
  apps: '#34d399',
  cards: '#fbbf24',
};

function TrendChart({ title, data, color, icon }: { title: string; data: TrendPoint[]; color: string; icon: React.ReactNode }) {
  return (
    <div className="card" style={{ padding: 20 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 16 }}>
        <span style={{ color }}>{icon}</span>
        <span style={{ fontWeight: 600, fontSize: 14 }}>{title}</span>
      </div>
      <ResponsiveContainer width="100%" height={200}>
        <LineChart data={data} margin={{ top: 5, right: 10, left: -20, bottom: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--border-light)" />
          <XAxis
            dataKey="date"
            tick={{ fontSize: 11, fill: 'var(--text-muted)' }}
            tickFormatter={(v: any) => String(v).slice(5)}
            axisLine={{ stroke: 'var(--border)' }}
            tickLine={false}
          />
          <YAxis
            allowDecimals={false}
            tick={{ fontSize: 11, fill: 'var(--text-muted)' }}
            axisLine={{ stroke: 'var(--border)' }}
            tickLine={false}
          />
          <Tooltip
            contentStyle={{
              background: 'var(--bg-card)',
              border: '1px solid var(--border)',
              borderRadius: 8,
              fontSize: 13,
            }}
            labelFormatter={(v: any) => String(v)}
            formatter={(value: any) => [value, '新增']}
          />
          <Line type="monotone" dataKey="count" stroke={color} strokeWidth={2} dot={false} activeDot={{ r: 4 }} />
        </LineChart>
      </ResponsiveContainer>
    </div>
  );
}

export default function AdminDashboard() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [loading, setLoading] = useState(true);
  const [health, setHealth] = useState<HealthStatus | null>(null);
  const [healthLoading, setHealthLoading] = useState(true);
  const [trends, setTrends] = useState<TrendData | null>(null);
  const [trendsLoading, setTrendsLoading] = useState(true);

  useEffect(() => {
    adminApi.getStats().then(res => {
      if (res.data.success) setStats(res.data.data);
    }).catch(() => {}).finally(() => setLoading(false));

    adminApi.getTrends().then(res => {
      if (res.data.success) setTrends(res.data.data);
    }).catch(() => {}).finally(() => setTrendsLoading(false));

    healthApi.check().then(res => {
      setHealth(res.data);
    }).catch(() => {
      setHealth({ status: 'degraded', db: 'error', redis: 'error', mq: 'error', uptime_secs: 0 });
    }).finally(() => setHealthLoading(false));
  }, []);

  const statCards = [
    { label: '注册商户', value: stats?.merchants ?? '—', icon: <Users size={18} />, color: '#7c6af7' },
    { label: '应用总数', value: stats?.total_apps ?? '—', icon: <Package size={18} />, color: '#34d399' },
    { label: '卡密总数', value: stats?.total_cards ?? '—', icon: <Key size={18} />, color: '#fbbf24' },
    { label: '活跃卡密', value: stats?.active_cards ?? '—', icon: <TrendingUp size={18} />, color: '#60a5fa' },
    { label: '激活次数', value: stats?.total_activations ?? '—', icon: <Activity size={18} />, color: '#f472b6' },
  ];

  const depItems = [
    { key: 'db'    as const, label: '数据库',   icon: <Database size={15} /> },
    { key: 'redis' as const, label: 'Redis',    icon: <Server size={15} /> },
    { key: 'mq'    as const, label: 'RabbitMQ', icon: <GitBranch size={15} /> },
  ];

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">平台总览</h1>
          <p className="page-subtitle">KamiSM 平台运行数据</p>
        </div>
      </div>

      <div className="stats-grid">
        {statCards.map(card => (
          <div key={card.label} className="stat-card" style={{ borderTopColor: card.color } as React.CSSProperties}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
              <span className="stat-label">{card.label}</span>
              <span style={{ color: card.color, opacity: 0.8 }}>{card.icon}</span>
            </div>
            {loading ? (
              <span className="skeleton" style={{ display: 'block', width: '60%', height: 32, borderRadius: 6 }} />
            ) : (
              <div className="stat-value data-enter" style={{ color: card.color }}>{String(card.value)}</div>
            )}
          </div>
        ))}
      </div>

      {/* ── 每日增量趋势 ── */}
      <div style={{ marginTop: 28 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 16 }}>
          <BarChart3 size={18} style={{ color: 'var(--text-muted)' }} />
          <h3 style={{ fontSize: 15, fontWeight: 700 }}>近 30 天每日增量</h3>
        </div>
        {trendsLoading ? (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 16 }}>
            {[1, 2, 3].map(i => (
              <div key={i} className="card" style={{ padding: 20 }}>
                <span className="skeleton" style={{ width: '40%', height: 18, display: 'block', marginBottom: 16 }} />
                <span className="skeleton" style={{ width: '100%', height: 200, display: 'block' }} />
              </div>
            ))}
          </div>
        ) : trends ? (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 16 }}>
            <TrendChart title="注册商户" data={trends.merchants} color={chartColors.merchants} icon={<Users size={15} />} />
            <TrendChart title="新增应用" data={trends.apps} color={chartColors.apps} icon={<Package size={15} />} />
            <TrendChart title="新增卡密" data={trends.cards} color={chartColors.cards} icon={<Key size={15} />} />
          </div>
        ) : null}
      </div>

      {/* ── 依赖健康状态 ── */}
      <div className="card" style={{ marginTop: 28 }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
          <span style={{ fontWeight: 600, fontSize: 14, color: 'var(--text)' }}>服务依赖状态</span>
          {healthLoading ? (
            <span className="skeleton" style={{ width: 60, height: 22, borderRadius: 20, display: 'inline-block' }} />
          ) : (
            <span style={{
              fontSize: 12,
              fontWeight: 600,
              padding: '3px 10px',
              borderRadius: 20,
              background: health?.status === 'ok' ? 'rgba(52,211,153,0.12)' : 'rgba(248,113,113,0.12)',
              color: health?.status === 'ok' ? '#34d399' : '#f87171',
            }}>
              {health?.status === 'ok' ? '全部正常' : '部分异常'}
            </span>
          )}
        </div>

        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 12 }}>
          {depItems.map(({ key, label, icon }) => {
            const ok = health?.[key] === 'ok';
            return (
              <div key={key} style={{
                display: 'flex', alignItems: 'center', gap: 10,
                padding: '10px 14px', borderRadius: 10,
                background: 'var(--bg)',
                border: `1px solid ${healthLoading ? 'var(--border)' : ok ? 'rgba(52,211,153,0.25)' : 'rgba(248,113,113,0.25)'}`,
              }}>
                <span style={{ color: healthLoading ? 'var(--text-muted)' : ok ? '#34d399' : '#f87171' }}>
                  {icon}
                </span>
                <span style={{ fontSize: 13, color: 'var(--text)', flex: 1 }}>{label}</span>
                {healthLoading ? (
                  <span className="skeleton" style={{ width: 36, height: 16, borderRadius: 4, display: 'inline-block' }} />
                ) : (
                  <span style={{ fontSize: 12, color: ok ? '#34d399' : '#f87171', fontWeight: 600 }}>
                    {ok ? 'OK' : 'ERROR'}
                  </span>
                )}
              </div>
            );
          })}
        </div>

        {!healthLoading && health && (
          <div style={{ marginTop: 12, fontSize: 12, color: 'var(--text-muted)' }}>
            运行时长：{formatUptime(health.uptime_secs)}
          </div>
        )}
      </div>
    </div>
  );
}
