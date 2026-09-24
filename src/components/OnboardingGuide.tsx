import { useState } from 'react';
import { Package, Key, Activity, BookOpen, X } from 'lucide-react';

export interface OnboardingStep {
  icon: React.ReactNode;
  title: string;
  desc: string;
}

/**
 * 商户首次使用引导。
 *
 * 判定「首次」：localStorage 键 `merchant_onboarded_<user.id>`。
 *   - 带 user.id，保证是「每商户每浏览器一次」，而非所有商户共享一个全局标记；
 *   - 用 localStorage 而非后端持久化：引导是纯产品体验，丢了（换浏览器/清缓存）
 *     代价极小（大不了再引导一次），不值得为此加数据库字段和迁移。
 * 跳过与「完成」都写标记，二者等价——用户看过一眼就算完成，不再打扰。
 */

const steps: OnboardingStep[] = [
  {
    icon: <Package size={28} />,
    title: '创建你的第一个应用',
    desc: '在「我的应用」里新建应用，每个应用会获得独立的 API Key，供你的软件接入卡密校验。',
  },
  {
    icon: <Key size={28} />,
    title: '生成并分发卡密',
    desc: '为应用批量生成卡密，按「未激活 / 已激活 / 已过期」筛选管理，可导出 CSV 方便分发。',
  },
  {
    icon: <Activity size={28} />,
    title: '实时追踪激活情况',
    desc: '「激活记录」实时展示每张卡密的激活设备、IP 归属地；「总览」里有客户 IP 分布地图。',
  },
  {
    icon: <BookOpen size={28} />,
    title: '按文档接入 SDK',
    desc: '「API 文档」提供完整接口说明，照着示例几行代码就能在你的软件里跑通卡密校验。',
  },
];

export default function OnboardingGuide({ onDone }: { onDone: () => void }) {
  const [idx, setIdx] = useState(0);
  const last = idx === steps.length - 1;

  const finish = () => onDone();
  const skip = () => onDone();
  const next = () => {
    if (last) finish();
    else setIdx((i) => i + 1);
  };
  const prev = () => setIdx((i) => Math.max(0, i - 1));

  return (
    <div className="modal-overlay" style={{ zIndex: 1100 }}>
      <div
        className="modal"
        style={{ maxWidth: 520, width: '92vw' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 顶部：欢迎 + 关闭 */}
        <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', marginBottom: 20 }}>
          <div>
            <div style={{ fontSize: 11, fontWeight: 700, letterSpacing: '0.6px', textTransform: 'uppercase', color: 'var(--accent)', marginBottom: 4 }}>
              欢迎使用 KamiSM
            </div>
            <h2 style={{ fontSize: 19, fontWeight: 800, margin: 0 }}>{steps[idx].title}</h2>
          </div>
          <button
            onClick={skip}
            aria-label="跳过引导"
            style={{
              background: 'none', border: 'none', cursor: 'pointer',
              color: 'var(--text-muted)', padding: 4, borderRadius: 6,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
            }}
          >
            <X size={18} />
          </button>
        </div>

        {/* 图标 + 说明 */}
        <div style={{ display: 'flex', gap: 16, alignItems: 'flex-start', marginBottom: 28 }}>
          <div style={{
            width: 64, height: 64, borderRadius: 16, flexShrink: 0,
            background: 'linear-gradient(135deg, rgba(124,106,247,0.18), rgba(90,78,209,0.12))',
            border: '1px solid rgba(124,106,247,0.25)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            color: 'var(--accent)',
          }}>
            {steps[idx].icon}
          </div>
          <p style={{ fontSize: 14, color: 'var(--text-dim)', lineHeight: 1.8, margin: 0, paddingTop: 4 }}>
            {steps[idx].desc}
          </p>
        </div>

        {/* 进度点 */}
        <div style={{ display: 'flex', gap: 6, justifyContent: 'center', marginBottom: 24 }}>
          {steps.map((_, i) => (
            <span
              key={i}
              style={{
                width: i === idx ? 20 : 6,
                height: 6,
                borderRadius: 3,
                background: i === idx ? 'var(--accent)' : 'var(--border)',
                transition: 'all 0.2s',
              }}
            />
          ))}
        </div>

        {/* 底部操作 */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          {idx > 0 ? (
            <button className="btn btn-ghost" style={{ fontSize: 13 }} onClick={prev}>
              上一步
            </button>
          ) : (
            <button className="btn btn-ghost" style={{ fontSize: 13 }} onClick={skip}>
              跳过
            </button>
          )}
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn btn-primary" style={{ fontSize: 13, padding: '8px 20px' }} onClick={next}>
              {last ? '开始使用' : '下一步'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
