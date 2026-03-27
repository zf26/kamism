import { useEffect, useState } from 'react';
import { adminApi } from '../../lib/api';
import { Users, Key, Activity, Package, TrendingUp } from 'lucide-react';

interface Stats {
  merchants: number;
  total_cards: number;
  active_cards: number;
  total_activations: number;
  total_apps: number;
}

export default function AdminDashboard() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    adminApi.getStats().then(res => {
      if (res.data.success) setStats(res.data.data);
    }).catch(() => {}).finally(() => setLoading(false));
  }, []);

  const statCards = [
    { label: '注册商户', value: stats?.merchants ?? '—', icon: <Users size={18} />, color: '#7c6af7' },
    { label: '应用总数', value: stats?.total_apps ?? '—', icon: <Package size={18} />, color: '#34d399' },
    { label: '卡密总数', value: stats?.total_cards ?? '—', icon: <Key size={18} />, color: '#fbbf24' },
    { label: '活跃卡密', value: stats?.active_cards ?? '—', icon: <TrendingUp size={18} />, color: '#60a5fa' },
    { label: '激活次数', value: stats?.total_activations ?? '—', icon: <Activity size={18} />, color: '#f472b6' },
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

      <div className="card">
        <div style={{ color: 'var(--text-muted)', fontSize: 13, lineHeight: 1.8 }}>
          <p style={{ marginBottom: 8 }}><strong style={{ color: 'var(--accent)' }}>管理员提示</strong></p>
          <p>• 前往「商户管理」查看所有注册商户，可禁用/启用账号</p>
          <p>• 初始管理员账号在 .env 文件中配置（ADMIN_EMAIL / ADMIN_PASSWORD）</p>
          <p>• 服务器默认监听端口 9527，可通过 PORT 环境变量修改</p>
        </div>
      </div>
    </div>
  );
}
