import { useEffect, useState } from 'react';
import { adminApi } from '../../lib/api';
import { Save, RefreshCw, Crown, Gift, Infinity } from 'lucide-react';
import toast from 'react-hot-toast';

interface PlanConfig {
  id: string;
  plan: string;
  label: string;
  max_apps: number;
  max_cards: number;
  max_devices: number;
  max_gen_once: number;
  updated_at: string;
}

interface EditState {
  label: string;
  max_apps: string;
  max_cards: string;
  max_devices: string;
  max_gen_once: string;
}

const FIELD_LABELS: { key: keyof EditState; label: string; hint: string }[] = [
  { key: 'label',        label: '套餐显示名称', hint: '如：免费版、专业版' },
  { key: 'max_apps',     label: '最多应用数',   hint: '-1 表示无限制' },
  { key: 'max_cards',    label: '最多卡密总数', hint: '-1 表示无限制' },
  { key: 'max_devices',  label: '单张卡密最多设备数', hint: '1-100，-1 表示无限制' },
  { key: 'max_gen_once', label: '单次最多生成卡密数', hint: '-1 表示无限制' },
];

function toEdit(c: PlanConfig): EditState {
  return {
    label:        c.label,
    max_apps:     String(c.max_apps),
    max_cards:    String(c.max_cards),
    max_devices:  String(c.max_devices),
    max_gen_once: String(c.max_gen_once),
  };
}

function displayVal(v: number) {
  return v === -1 ? '无限' : String(v);
}

export default function PlanConfigs() {
  const [configs, setConfigs] = useState<PlanConfig[]>([]);
  const [edits, setEdits] = useState<Record<string, EditState>>({});
  const [saving, setSaving] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = () => {
    setLoading(true);
    adminApi.getPlanConfigs()
      .then(res => {
        if (res.data.success) {
          const data: PlanConfig[] = res.data.data;
          setConfigs(data);
          const initEdits: Record<string, EditState> = {};
          data.forEach(c => { initEdits[c.id] = toEdit(c); });
          setEdits(initEdits);
        }
      })
      .finally(() => setLoading(false));
  };

  useEffect(() => { load(); }, []);

  const handleChange = (id: string, field: keyof EditState, val: string) => {
    setEdits(prev => ({ ...prev, [id]: { ...prev[id], [field]: val } }));
  };

  const handleSave = async (config: PlanConfig) => {
    const e = edits[config.id];
    if (!e) return;
    setSaving(config.id);
    try {
      const payload = {
        label:        e.label || undefined,
        max_apps:     e.max_apps     !== '' ? Number(e.max_apps)     : undefined,
        max_cards:    e.max_cards    !== '' ? Number(e.max_cards)    : undefined,
        max_devices:  e.max_devices  !== '' ? Number(e.max_devices)  : undefined,
        max_gen_once: e.max_gen_once !== '' ? Number(e.max_gen_once) : undefined,
      };
      const res = await adminApi.updatePlanConfig(config.id, payload);
      if (res.data.success) {
        toast.success(`${config.label} 配置已保存`);
        load();
      } else {
        toast.error(res.data.message || '保存失败');
      }
    } catch {
      toast.error('保存失败');
    } finally {
      setSaving(null);
    }
  };

  const isPro = (plan: string) => plan === 'pro';

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">套餐配置</h1>
          <p className="page-subtitle">管理各订阅计划的功能限制，修改后实时生效</p>
        </div>
        <button className="btn btn-ghost" onClick={load}>
          <RefreshCw size={14} /> 刷新
        </button>
      </div>

      {loading ? (
        <div style={{ textAlign: 'center', padding: 60, color: 'var(--text-muted)' }}>
          <span className="spinner" />
        </div>
      ) : (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(340px, 1fr))', gap: 20 }}>
          {configs.map(config => {
            const e = edits[config.id];
            if (!e) return null;
            return (
              <div key={config.id} className="card" style={{
                borderColor: isPro(config.plan) ? 'rgba(124,58,237,0.35)' : 'var(--border)',
                background: isPro(config.plan) ? 'linear-gradient(135deg, rgba(124,58,237,0.06) 0%, var(--surface) 60%)' : 'var(--surface)',
              }}>
                {/* 卡片头部 */}
                <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 24, paddingBottom: 16, borderBottom: '1px solid var(--border)' }}>
                  <span style={{
                    width: 40, height: 40, borderRadius: 10,
                    display: 'flex', alignItems: 'center', justifyContent: 'center',
                    background: isPro(config.plan) ? 'rgba(124,58,237,0.15)' : 'rgba(255,255,255,0.06)',
                    border: `1px solid ${isPro(config.plan) ? 'rgba(124,58,237,0.3)' : 'var(--border)'}`,
                  }}>
                    {isPro(config.plan)
                      ? <Crown size={18} color="#a78bfa" />
                      : <Gift size={18} color="var(--text-muted)" />
                    }
                  </span>
                  <div>
                    <h3 style={{ fontWeight: 700, fontSize: 16, color: isPro(config.plan) ? '#a78bfa' : 'var(--text)' }}>
                      {config.label}
                    </h3>
                    <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
                      plan: <code style={{ fontFamily: 'var(--mono)' }}>{config.plan}</code>
                      {' · '}最后修改 {new Date(config.updated_at).toLocaleString('zh-CN')}
                    </p>
                  </div>
                </div>

                {/* 当前值展示 */}
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginBottom: 20 }}>
                  {[['应用', config.max_apps], ['卡密', config.max_cards], ['设备/张', config.max_devices], ['单次生成', config.max_gen_once]].map(([label, val]) => (
                    <span key={label as string} style={{
                      display: 'inline-flex', alignItems: 'center', gap: 4,
                      fontSize: 12, padding: '3px 10px', borderRadius: 999,
                      background: 'rgba(255,255,255,0.04)', border: '1px solid var(--border)',
                      color: 'var(--text-muted)',
                    }}>
                      {(val as number) === -1 && <Infinity size={11} />}
                      {label as string}: {displayVal(val as number)}
                    </span>
                  ))}
                </div>

                {/* 编辑表单 */}
                <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
                  {FIELD_LABELS.map(({ key, label, hint }) => (
                    <div key={key}>
                      <label style={{ fontSize: 12, color: 'var(--text-muted)', display: 'block', marginBottom: 4 }}>
                        {label}
                        <span style={{ marginLeft: 6, color: 'var(--text-muted)', opacity: 0.6 }}>({hint})</span>
                      </label>
                      <input
                        value={e[key]}
                        onChange={ev => handleChange(config.id, key, ev.target.value)}
                        style={{ width: '100%' }}
                        type={key === 'label' ? 'text' : 'number'}
                      />
                    </div>
                  ))}
                </div>

                <button
                  className="btn btn-primary"
                  style={{ width: '100%', justifyContent: 'center', marginTop: 20 }}
                  onClick={() => handleSave(config)}
                  disabled={saving === config.id}
                >
                  {saving === config.id
                    ? <><span className="spinner" style={{ width: 14, height: 14 }} /> 保存中…</>
                    : <><Save size={14} /> 保存配置</>
                  }
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

