import { useEffect, useState } from 'react';
import { cardsApi, activationsApi } from '../../lib/api';
import { Key, Activity } from 'lucide-react';

export default function MerchantDashboard() {
  const [stats, setStats] = useState({ total_cards: 0, active_cards: 0, total_apps: 0, total_activations: 0 });

  useEffect(() => {
    Promise.all([
      cardsApi.list({ page_size: 1 }),
      activationsApi.list({ page_size: 1 }),
    ]).then(([cardsRes, activationsRes]) => {
      setStats(prev => ({
        ...prev,
        total_cards: cardsRes.data.total ?? 0,
        total_activations: activationsRes.data.total ?? 0,
      }));
    }).catch(() => {});
  }, []);

  const statCards = [
    { label: '卡密总数', value: stats.total_cards, icon: <Key size={18} />, color: '#7c6af7' },
    { label: '激活记录', value: stats.total_activations, icon: <Activity size={18} />, color: '#34d399' },
  ];

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">控制台</h1>
          <p className="page-subtitle">欢迎使用 KamiSM 卡密管理平台</p>
        </div>
      </div>

      <div className="stats-grid">
        {statCards.map(card => (
          <div key={card.label} className="stat-card">
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
              <span className="stat-label">{card.label}</span>
              <span style={{ color: card.color, opacity: 0.8 }}>{card.icon}</span>
            </div>
            <div className="stat-value" style={{ color: card.color }}>{card.value}</div>
          </div>
        ))}
      </div>

      <div className="card">
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

