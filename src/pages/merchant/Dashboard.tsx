import { useEffect, useState } from 'react';
import { merchantApi } from '../../lib/api';
import { Key, Activity, Package, Monitor } from 'lucide-react';
import {
  ResponsiveContainer,
  LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip,
  PieChart, Pie, Cell, Legend,
  BarChart, Bar,
} from 'recharts';
import { useThemeStore } from '../../stores/theme';

interface DashboardStats {
  card_stats: { status: string; count: number }[];
  activation_trend: { date: string; count: number }[];
  device_dist: { app: string; count: number }[];
}

const STATUS_LABEL: Record<string, string> = {
  unused: '未使用', active: '使用中', expired: '已过期', disabled: '已禁用',
};
const STATUS_COLOR: Record<string, string> = {
  unused: '#888899', active: '#7c6af7', expired: '#f87171', disabled: '#fbbf24',
};

type Range = 'week' | 'month' | 'year';
const RANGE_LABELS: Record<Range, string> = { week: '近7天', month: '近3月', year: '近1年' };
const RANGE_TICK_FORMAT: Record<Range, (d: string) => string> = {
  week:  (d) => d.slice(5),         // MM-DD
  month: (d) => d.slice(5),         // MM-DD（按周截断，显示周一日期）
  year:  (d) => d.slice(0, 7),      // YYYY-MM
};

export default function MerchantDashboard() {
  const [stats, setStats] = useState<DashboardStats>({
    card_stats: [],
    activation_trend: [],
    device_dist: [],
  });
  const [loading, setLoading] = useState(true);
  const [range, setRange] = useState<Range>('week');
  const { theme } = useThemeStore();

  const axisColor = theme === 'dark' ? '#55556a' : '#8888a0';
  const gridColor = theme === 'dark' ? '#1e1e2e' : '#dddde8';
  const tooltipBg = theme === 'dark' ? '#111118' : '#ffffff';
  const tooltipBorder = theme === 'dark' ? '#2a2a3e' : '#c8c8da';
  const tooltipText = theme === 'dark' ? '#e8e8f0' : '#18181f';

  useEffect(() => {
    setLoading(true);
    merchantApi.dashboardStats(range)
      .then(res => { if (res.data.success) setStats(res.data.data); })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [range]);

  const totalCards = stats.card_stats.reduce((s, c) => s + c.count, 0);
  const activeCards = stats.card_stats.find(c => c.status === 'active')?.count ?? 0;
  const totalActivations = stats.activation_trend.reduce((s, c) => s + c.count, 0);
  const totalDevices = stats.device_dist.reduce((s, c) => s + c.count, 0);

  const summaryCards = [
    { label: '卡密总数', value: totalCards, icon: <Key size={18} />, color: 'var(--accent)' },
    { label: '使用中', value: activeCards, icon: <Activity size={18} />, color: 'var(--success)' },
    { label: '近30天激活', value: totalActivations, icon: <Monitor size={18} />, color: '#f472b6' },
    { label: '绑定设备', value: totalDevices, icon: <Package size={18} />, color: 'var(--warning)' },
  ];

  const tooltipStyle = {
    background: tooltipBg,
    border: `1px solid ${tooltipBorder}`,
    borderRadius: 8,
    color: tooltipText,
    fontSize: 12,
  };

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">控制台</h1>
          <p className="page-subtitle">欢迎使用 KamiSM 卡密管理平台</p>
        </div>
      </div>

      {/* 概要数据 */}
      <div className="stats-grid">
        {summaryCards.map(card => (
          <div key={card.label} className="stat-card">
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
              <span className="stat-label">{card.label}</span>
              <span style={{ color: card.color, opacity: 0.8 }}>{card.icon}</span>
            </div>
            <div className="stat-value" style={{ color: card.color }}>{card.value}</div>
          </div>
        ))}
      </div>

      {loading ? (
        <div style={{ textAlign: 'center', padding: 60 }}><span className="spinner" /></div>
      ) : (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(340px, 1fr))', gap: 20 }}>

          {/* 激活趋势折线图 */}
          <div className="card" style={{ gridColumn: '1 / -1' }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
              <p style={{ fontWeight: 700, color: 'var(--text)', fontSize: 14, letterSpacing: '-0.2px', margin: 0 }}>
                激活趋势
              </p>
              <div style={{ display: 'flex', gap: 4 }}>
                {(['week', 'month', 'year'] as Range[]).map(r => (
                  <button
                    key={r}
                    onClick={() => setRange(r)}
                    style={{
                      padding: '4px 12px',
                      borderRadius: 6,
                      fontSize: 12,
                      fontWeight: 600,
                      border: '1px solid',
                      cursor: 'pointer',
                      background: range === r ? 'var(--accent)' : 'transparent',
                      color: range === r ? '#fff' : 'var(--text-dim)',
                      borderColor: range === r ? 'var(--accent)' : 'var(--border-light)',
                      transition: 'all 0.15s',
                    }}
                  >
                    {RANGE_LABELS[r]}
                  </button>
                ))}
              </div>
            </div>
            {stats.activation_trend.length === 0 ? (
              <div className="empty-state" style={{ padding: '40px 0' }}>
                <div className="empty-state-icon">📈</div>
                <div className="empty-state-text">暂无激活数据</div>
              </div>
            ) : (
              <ResponsiveContainer width="100%" height={220}>
                <LineChart data={stats.activation_trend} margin={{ top: 4, right: 16, left: -20, bottom: 0 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke={gridColor} />
                  <XAxis
                    dataKey="date"
                    tick={{ fontSize: 11, fill: axisColor }}
                    tickFormatter={RANGE_TICK_FORMAT[range]}
                  />
                  <YAxis tick={{ fontSize: 11, fill: axisColor }} allowDecimals={false} />
                  <Tooltip
                    contentStyle={tooltipStyle}
                    formatter={(v) => [Number(v ?? 0), '激活次数']}
                    labelFormatter={l => `日期：${l}`}
                  />
                  <Line
                    type="monotone"
                    dataKey="count"
                    stroke="var(--accent)"
                    strokeWidth={2}
                    dot={{ r: 3, fill: 'var(--accent)' }}
                    activeDot={{ r: 5 }}
                  />
                </LineChart>
              </ResponsiveContainer>
            )}
          </div>

          {/* 卡密状态饼图 */}
          <div className="card">
            <p style={{ fontWeight: 700, marginBottom: 20, color: 'var(--text)', fontSize: 14 }}>卡密使用率</p>
            {stats.card_stats.length === 0 ? (
              <div className="empty-state" style={{ padding: '40px 0' }}>
                <div className="empty-state-icon">🔑</div>
                <div className="empty-state-text">暂无卡密</div>
              </div>
            ) : (
              <ResponsiveContainer width="100%" height={220}>
                <PieChart>
                  <Pie
                    data={stats.card_stats}
                    dataKey="count"
                    nameKey="status"
                    cx="50%" cy="50%"
                    innerRadius={55}
                    outerRadius={85}
                    paddingAngle={3}
                  >
                    {stats.card_stats.map((entry) => (
                      <Cell key={entry.status} fill={STATUS_COLOR[entry.status] ?? '#888'} />
                    ))}
                  </Pie>
                  <Tooltip
                    contentStyle={tooltipStyle}
                    formatter={(v, _, props) => {
                      const status = (props as { payload?: { status?: string } }).payload?.status ?? '';
                      return [Number(v ?? 0), STATUS_LABEL[status] ?? status];
                    }}
                  />
                  <Legend
                    formatter={(value: string) => STATUS_LABEL[value] ?? value}
                    wrapperStyle={{ fontSize: 12, color: axisColor }}
                  />
                </PieChart>
              </ResponsiveContainer>
            )}
          </div>

          {/* 设备分布柱状图 */}
          <div className="card">
            <p style={{ fontWeight: 700, marginBottom: 20, color: 'var(--text)', fontSize: 14 }}>应用设备分布</p>
            {stats.device_dist.length === 0 ? (
              <div className="empty-state" style={{ padding: '40px 0' }}>
                <div className="empty-state-icon">📱</div>
                <div className="empty-state-text">暂无设备数据</div>
              </div>
            ) : (
              <ResponsiveContainer width="100%" height={220}>
                <BarChart data={stats.device_dist} margin={{ top: 4, right: 16, left: -20, bottom: 0 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke={gridColor} />
                  <XAxis
                    dataKey="app"
                    tick={{ fontSize: 11, fill: axisColor }}
                    tickFormatter={s => s.length > 8 ? s.slice(0, 8) + '…' : s}
                  />
                  <YAxis tick={{ fontSize: 11, fill: axisColor }} allowDecimals={false} />
                  <Tooltip
                    contentStyle={tooltipStyle}
                    formatter={(v) => [Number(v ?? 0), '绑定设备数']}
                    labelFormatter={l => `应用：${l}`}
                  />
                  <Bar dataKey="count" fill="var(--accent)" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            )}
          </div>

        </div>
      )}

      {/* 快速开始 */}
      <div className="card" style={{ marginTop: 20 }}>
        <p style={{ fontWeight: 700, marginBottom: 12, color: 'var(--text)' }}>快速开始</p>
        <div style={{ color: 'var(--text-muted)', fontSize: 13, lineHeight: 2 }}>
          <p>1. 前往「我的应用」创建一个应用</p>
          <p>2. 前往「卡密管理」批量生成卡密</p>
          <p>3. 在「账号设置」中查看 API Key</p>
          <p>4. 在你的软件中调用 <span className="mono" style={{ color: 'var(--accent)' }}>POST /api/v1/activate</span> 激活卡密</p>
          <p>5. 每次软件启动时调用 <span className="mono" style={{ color: 'var(--accent)' }}>POST /api/v1/verify</span> 验证卡密</p>
        </div>
      </div>
    </div>
  );
}
