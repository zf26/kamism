import { useNavigate, useLocation } from 'react-router-dom';
import { useAuthStore } from '../stores/auth';
import {
  LayoutDashboard, Package, Key, Activity, Users,
  Settings, LogOut, Zap, Shield
} from 'lucide-react';

interface NavItem {
  label: string;
  path: string;
  icon: React.ReactNode;
}

const adminNav: NavItem[] = [
  { label: '总览', path: '/admin/dashboard', icon: <LayoutDashboard size={16} /> },
  { label: '商户管理', path: '/admin/merchants', icon: <Users size={16} /> },
];

const merchantNav: NavItem[] = [
  { label: '总览', path: '/dashboard', icon: <LayoutDashboard size={16} /> },
  { label: '我的应用', path: '/apps', icon: <Package size={16} /> },
  { label: '卡密管理', path: '/cards', icon: <Key size={16} /> },
  { label: '激活记录', path: '/activations', icon: <Activity size={16} /> },
  { label: '账号设置', path: '/settings', icon: <Settings size={16} /> },
];

export default function Layout({ children }: { children: React.ReactNode }) {
  const navigate = useNavigate();
  const location = useLocation();
  const { user, role, logout } = useAuthStore();

  const navItems = role === 'admin' ? adminNav : merchantNav;

  const handleLogout = () => {
    logout();
    navigate('/login');
  };

  return (
    <div style={{ display: 'flex', height: '100vh', overflow: 'hidden' }}>
      {/* Sidebar */}
      <aside style={{
        width: 220,
        minWidth: 220,
        background: 'var(--bg-card)',
        borderRight: '1px solid var(--border)',
        display: 'flex',
        flexDirection: 'column',
        padding: '20px 0',
      }}>
        {/* Logo */}
        <div style={{ padding: '0 20px 24px', borderBottom: '1px solid var(--border)', marginBottom: 12 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <div style={{
              width: 32, height: 32,
              background: 'linear-gradient(135deg, var(--accent), #a78bfa)',
              borderRadius: 8,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
            }}>
              <Zap size={16} color="#fff" />
            </div>
            <div>
              <div style={{ fontWeight: 800, fontSize: 15, letterSpacing: '-0.3px' }}>KamiSM</div>
              <div style={{ fontSize: 10, color: 'var(--text-muted)', letterSpacing: '0.5px', textTransform: 'uppercase' }}>
                {role === 'admin' ? '平台管理' : '商户控制台'}
              </div>
            </div>
          </div>
        </div>

        {/* Nav */}
        <nav style={{ flex: 1, padding: '0 12px' }}>
          {navItems.map((item) => {
            const active = location.pathname === item.path;
            return (
              <button
                key={item.path}
                onClick={() => navigate(item.path)}
                style={{
                  width: '100%',
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  padding: '10px 12px',
                  borderRadius: 8,
                  marginBottom: 2,
                  background: active ? 'var(--accent-glow)' : 'transparent',
                  color: active ? 'var(--accent)' : 'var(--text-dim)',
                  fontWeight: active ? 700 : 500,
                  fontSize: 13,
                  border: active ? '1px solid rgba(124,106,247,0.2)' : '1px solid transparent',
                  textAlign: 'left',
                  cursor: 'pointer',
                  transition: 'all 0.15s',
                }}
                onMouseEnter={e => { if (!active) (e.currentTarget as HTMLButtonElement).style.background = 'var(--bg-hover)'; }}
                onMouseLeave={e => { if (!active) (e.currentTarget as HTMLButtonElement).style.background = 'transparent'; }}
              >
                {item.icon}
                {item.label}
              </button>
            );
          })}
        </nav>

        {/* User info */}
        <div style={{ padding: '16px 20px', borderTop: '1px solid var(--border)' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
            <div style={{
              width: 32, height: 32, borderRadius: '50%',
              background: 'linear-gradient(135deg, var(--accent-dim), #6d28d9)',
              display: 'flex', alignItems: 'center', justifyContent: 'center',
              fontSize: 13, fontWeight: 700, color: '#fff', flexShrink: 0,
            }}>
              {user?.username?.[0]?.toUpperCase() ?? 'U'}
            </div>
            <div style={{ minWidth: 0 }}>
              <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--text)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {user?.username}
              </div>
              <div style={{ fontSize: 11, color: 'var(--text-muted)', display: 'flex', alignItems: 'center', gap: 3 }}>
                {role === 'admin' && <Shield size={10} />}
                {role === 'admin' ? '管理员' : '商户'}
              </div>
            </div>
          </div>
          <button className="btn btn-ghost" style={{ width: '100%', justifyContent: 'center', fontSize: 12 }} onClick={handleLogout}>
            <LogOut size={13} /> 退出登录
          </button>
        </div>
      </aside>

      {/* Main */}
      <main style={{ flex: 1, overflow: 'auto', padding: '32px 36px' }} className="fade-in">
        {children}
      </main>
    </div>
  );
}
