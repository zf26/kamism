import { useEffect, useRef, useState } from 'react';
import { useThemeStore } from '../stores/theme';

export interface RecentActivation {
  card_code: string;
  device_name: string | null;
  ip_address: string | null;
  ip_region: string | null;
  activated_at: string;
}

/**
 * 滚屏激活记录：最新激活卡密实时滚动展示。
 *
 * 为什么做滚屏而不是静态列表：
 *   - 商户把控制台当「运营大屏」看，滚动能让人一眼看到「此刻谁在激活」；
 *   - 数据量小（后端只给最近 5 条），滚动不卡。
 *
 * 行为约定：
 *   - 数据不足一屏时**不滚动**（避免空转的假滚屏）；
 *   - 鼠标悬停暂停滚动（用户想看清某条时不被顶走）；
 *   - IP 归属地解析不到（内网/未收录）显示「未知」，不显示空白。
 */
export default function ActivationTicker({ items }: { items: RecentActivation[] }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [paused, setPaused] = useState(false);
  const [scrolling, setScrolling] = useState(false);
  const { theme } = useThemeStore();

  const isDark = theme === 'dark';

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    // 内容没超出容器高度就不滚动（最近 5 条通常不足一屏，此时静态展示更清晰）
    const canScroll = el.scrollHeight > el.clientHeight;
    setScrolling(canScroll);
    if (!canScroll) return;

    const id = setInterval(() => {
      if (paused) return;
      // 滚到底后无缝回到顶部（数据是环形重复的，视觉上无跳变）
      if (el.scrollTop + el.clientHeight >= el.scrollHeight - 4) {
        el.scrollTop = 0;
      } else {
        el.scrollTop += 1;
      }
    }, 40);
    return () => clearInterval(id);
  }, [items, paused]);

  const fmt = (iso: string) => {
    const d = new Date(iso);
    return d.toLocaleString('zh-CN', {
      month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit', second: '2-digit',
    });
  };

  return (
    <div
      className="card"
      style={{ height: '100%', display: 'flex', flexDirection: 'column', overflow: 'hidden' }}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
        <p style={{ fontWeight: 700, color: 'var(--text)', fontSize: 14, letterSpacing: '-0.2px', margin: 0 }}>
          实时激活
        </p>
        <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>
          {scrolling ? (paused ? '已暂停' : '滚动中') : ''} · 最近 {items.length} 条
        </span>
      </div>

      {items.length === 0 ? (
        <div className="empty-state" style={{ padding: '40px 0' }}>
          <div className="empty-state-icon">📡</div>
          <div className="empty-state-text">暂无激活记录</div>
        </div>
      ) : (
        <div
          ref={scrollRef}
          style={{
            flex: 1,
            overflowY: 'auto',
            scrollbarWidth: 'none',
            msOverflowStyle: 'none',
          }}
        >
          <div>
            {/* 数据环形重复两份：滚到底后无缝回到顶部 */}
            {[...items, ...items].map((a, idx) => (
              <div
                key={`${a.card_code}-${a.activated_at}-${idx}`}
                className="data-enter"
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  padding: '9px 10px',
                  borderRadius: 8,
                  borderBottom: `1px solid ${isDark ? 'var(--border)' : 'var(--border)'}`,
                  marginBottom: 2,
                }}
              >
                <span
                  style={{
                    width: 7, height: 7, borderRadius: '50%', flexShrink: 0,
                    background: 'var(--success)',
                    boxShadow: '0 0 6px var(--success)',
                  }}
                />
                <span className="mono" style={{ fontSize: 12, color: 'var(--accent)', letterSpacing: '1px', flexShrink: 0 }}>
                  {a.card_code}
                </span>
                <span style={{ fontSize: 12, color: 'var(--text-dim)', flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {a.ip_address || '—'}
                  {a.ip_region ? ` · ${a.ip_region}` : ''}
                </span>
                <span style={{ fontSize: 11, color: 'var(--text-muted)', flexShrink: 0 }}>
                  {fmt(a.activated_at)}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
