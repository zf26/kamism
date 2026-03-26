import { useState, useEffect } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { useAuthStore } from '../stores/auth';
import { useThemeStore } from '../stores/theme';
import {
  LayoutDashboard, Package, Key, Activity, Users,
  Settings, LogOut, Shield, X, Bell, Megaphone, Sun, Moon
} from 'lucide-react';
import appIcon from '../assets/app-icon.png';
import { merchantMessagesApi } from '../lib/api';
import { useWs } from '../hooks/useWs';
import { useWsEventStore } from '../stores/wsEvent';

interface NavItem {
  label: string;
  path: string;
  icon: React.ReactNode;
}

const adminNav: NavItem[] = [
  { label: '总览', path: '/admin/dashboard', icon: <LayoutDashboard size={16} /> },
  { label: '商户管理', path: '/admin/merchants', icon: <Users size={16} /> },
  { label: '套餐配置', path: '/admin/plan-configs', icon: <Settings size={16} /> },
  { label: '消息管理', path: '/admin/messages', icon: <Megaphone size={16} /> },
];

const merchantNav: NavItem[] = [
  { label: '总览', path: '/dashboard', icon: <LayoutDashboard size={16} /> },
  { label: '我的应用', path: '/apps', icon: <Package size={16} /> },
  { label: '卡密管理', path: '/cards', icon: <Key size={16} /> },
  { label: '激活记录', path: '/activations', icon: <Activity size={16} /> },
  { label: '消息中心', path: '/messages', icon: <Bell size={16} /> },
  { label: '账号设置', path: '/settings', icon: <Settings size={16} /> },
];

export default function Layout({ children }: { children: React.ReactNode }) {
  const navigate = useNavigate();
  const location = useLocation();
  const { user, role, logout } = useAuthStore();
  const { theme, toggle: toggleTheme } = useThemeStore();
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [unread, setUnread] = useState(0);
  const [noticeQueue, setNoticeQueue] = useState<{id:string;title:string;content:string;created_at:string}[]>([]);
  const setLastEvent = useWsEventStore((s) => s.setLastEvent);

  // 商户端：拉取未读站内信数
  useEffect(() => {
    if (role !== 'merchant') return;
    merchantMessagesApi.unreadCount()
      .then((res) => { if (res.data.success) setUnread(res.data.data.unread); })
      .catch(() => {});
  }, [role, location.pathname]);

  // 商户端：登录后拉取最新公告，用 localStorage 永久记录已读，只弹未读过的
  useEffect(() => {
    if (role !== 'merchant') return;
    merchantMessagesApi.listNotices({ page: 1, page_size: 5 })
      .then((res) => {
        if (!res.data.success) return;
        const shown: string[] = JSON.parse(localStorage.getItem('shown_notices') || '[]');
        const pending = (res.data.data as {id:string;title:string;content:string;created_at:string}[])
          .filter((n) => !shown.includes(n.id));
        if (pending.length > 0) setNoticeQueue(pending);
      })
      .catch(() => {});
  }, [role]);

  // 仅商户端建立 WebSocket 连接，管理员不需要
  useWs({
    onMessage: (evt) => {
      if (role !== 'merchant') return;
      setLastEvent(evt);
      if (evt.event === 'new_message') {
        setUnread((n) => n + 1);
      }
    },
    // 管理员完全禁用 WS（-1 = 不建立初始连接）
    reconnectInterval: role === 'merchant' ? 3000 : -1,
  });

  // 确认当前公告已读（localStorage 永久记录），弹出下一条
  const handleNoticeConfirm = () => {
    const [current, ...rest] = noticeQueue;
    if (current) {
      const shown: string[] = JSON.parse(localStorage.getItem('shown_notices') || '[]');
      localStorage.setItem('shown_notices', JSON.stringify([...shown, current.id]));
    }
    setNoticeQueue(rest);
  };

  const navItems = role === 'admin' ? adminNav : merchantNav;

  const handleLogout = () => {
    logout();
    navigate('/login');
  };

  const handleNav = (path: string) => {
    navigate(path);
    setSidebarOpen(false);
  };

  const SidebarContent = () => (
    <>
      {/* Logo */}
      <div style={{ padding: '0 20px 24px', borderBottom: '1px solid var(--border)', marginBottom: 12 }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <img
              src={appIcon}
              alt="KamiSM"
              style={{ width: 32, height: 32, borderRadius: 8, objectFit: 'cover' }}
            />
            <div>
              <div style={{ fontWeight: 800, fontSize: 15, letterSpacing: '-0.3px' }}>KamiSM</div>
              <div style={{ fontSize: 10, color: 'var(--text-muted)', letterSpacing: '0.5px', textTransform: 'uppercase' }}>
                {role === 'admin' ? '平台管理' : '商户控制台'}
              </div>
            </div>
          </div>
          {/* 移动端关闭按钮 */}
          <button
            onClick={() => setSidebarOpen(false)}
            style={{
              background: 'none', border: 'none', color: 'var(--text-muted)',
              cursor: 'pointer', padding: 4, borderRadius: 6,
              display: 'none',
            }}
            className="sidebar-close-btn"
          >
            <X size={18} />
          </button>
        </div>
      </div>

      {/* Nav */}
      <nav style={{ flex: 1, padding: '0 12px' }}>
        {navItems.map((item) => {
          const active = location.pathname === item.path;
          return (
            <button
              key={item.path}
              onClick={() => handleNav(item.path)}
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
              <span style={{ flex: 1 }}>{item.label}</span>
              {item.path === '/messages' && unread > 0 && (
                <span style={{
                  background: 'var(--accent)',
                  color: '#fff',
                  borderRadius: 10,
                  fontSize: 10,
                  fontWeight: 700,
                  padding: '1px 6px',
                  minWidth: 18,
                  textAlign: 'center',
                  lineHeight: '16px',
                }}>
                  {unread > 99 ? '99+' : unread}
                </span>
              )}
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
        <div style={{ display: 'flex', gap: 6, marginBottom: 8 }}>
          <button
            className="btn btn-ghost"
            style={{ flex: 1, justifyContent: 'center', fontSize: 12 }}
            onClick={toggleTheme}
            title={theme === 'dark' ? '切换亮色' : '切换暗色'}
          >
            {theme === 'dark' ? <Sun size={13} /> : <Moon size={13} />}
            {theme === 'dark' ? '亮色' : '暗色'}
          </button>
          <button
            className="btn btn-ghost"
            style={{ flex: 1, justifyContent: 'center', fontSize: 12 }}
            onClick={handleLogout}
          >
            <LogOut size={13} /> 退出
          </button>
        </div>
      </div>
    </>
  );

  return (
    <div style={{ display: 'flex', height: '100vh', overflow: 'hidden' }}>

      {/* ── 移动端顶部 Header ── */}
      <header className="mobile-header">
        <div className="mobile-header-logo">
          <img src={appIcon} alt="KamiSM" style={{ width: 28, height: 28, borderRadius: 7 }} />
          KamiSM
        </div>
        <button className="hamburger" onClick={() => setSidebarOpen(true)} aria-label="打开菜单">
          <span /><span /><span />
        </button>
      </header>

      {/* ── 移动端遮罩 ── */}
      <div
        className={`sidebar-overlay${sidebarOpen ? ' open' : ''}`}
        onClick={() => setSidebarOpen(false)}
      />

      {/* ── Sidebar ── */}
      <aside
        className={`layout-sidebar${sidebarOpen ? ' open' : ''}`}
        style={{
          width: 'var(--sidebar-w)',
          minWidth: 'var(--sidebar-w)',
          background: 'var(--bg-card)',
          borderRight: '1px solid var(--border)',
          display: 'flex',
          flexDirection: 'column',
          padding: '20px 0',
          overflowY: 'auto',
        }}
      >
        <SidebarContent />
      </aside>

      {/* ── Main ── */}
      <main
        className="layout-main fade-in"
        style={{
          flex: 1,
          overflow: 'auto',
          padding: '32px 36px',
        }}
      >
        {children}
      </main>

      {/* ── 公告弹窗（session 内每条只弹一次）── */}
      {noticeQueue.length > 0 && (
        <div className="modal-overlay" style={{ zIndex: 1050 }}>
          <div className="modal" style={{ maxWidth: 640, width: '90vw' }} onClick={(e) => e.stopPropagation()}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 8 }}>
              <div style={{
                width: 40, height: 40, borderRadius: 10, flexShrink: 0,
                background: 'rgba(124,106,247,0.12)',
                display: 'flex', alignItems: 'center', justifyContent: 'center',
              }}>
                <Megaphone size={20} style={{ color: 'var(--accent)' }} />
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 11, fontWeight: 700, letterSpacing: '0.6px', textTransform: 'uppercase', color: 'var(--text-muted)', marginBottom: 3 }}>平台公告</div>
                <h2 style={{ fontSize: 17, fontWeight: 800, margin: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{noticeQueue[0].title}</h2>
              </div>
            </div>
            <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 16, paddingLeft: 52 }}>
              {new Date(noticeQueue[0].created_at).toLocaleString('zh-CN')}
            </div>
            <div style={{
              fontSize: 14, color: 'var(--text-dim)', lineHeight: 1.9,
              whiteSpace: 'pre-wrap', wordBreak: 'break-word',
              marginBottom: 24, maxHeight: 360, overflowY: 'auto',
              padding: '14px 16px',
              background: 'var(--bg)',
              borderRadius: 8,
              border: '1px solid var(--border)',
            }}>
              {noticeQueue[0].content}
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              {noticeQueue.length > 1 ? (
                <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>还有 {noticeQueue.length - 1} 条公告未读</span>
              ) : <span />}
              <button className="btn btn-primary" onClick={handleNoticeConfirm}>
                {noticeQueue.length > 1 ? '下一条 →' : '我已知晓'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
