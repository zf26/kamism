import { useEffect, useState } from 'react';
import { agentApi } from '../../lib/api';
import { Plus, Trash2, RefreshCw, Users, TrendingUp, Copy, Network } from 'lucide-react';
import toast from 'react-hot-toast';
import { useConfirm } from '../../stores/confirm';

interface AgentRow {
  id: string;
  agent_id: string;
  agent_username: string;
  quota_total: number;
  quota_used: number;
  commission_rate: number;
  status: string;
  invite_code: string;
  note: string | null;
  created_at: string;
}

interface PendingInvite {
  invite_code: string;
  quota_total: number;
  commission_rate: number;
  created_at: string;
}

interface CommissionLog {
  id: string;
  agent_id: string;
  agent_username: string;
  commission_rate: number;
  units: number;
  created_at: string;
}

interface MyRelation {
  id: string;
  parent_id: string;
  parent_username: string;
  quota_total: number;
  quota_used: number;
  commission_rate: number;
  status: string;
  created_at: string;
}

export default function Agents() {
  const [tab, setTab] = useState<'my_agents' | 'commissions' | 'my_relation'>('my_agents');

  const [agents, setAgents] = useState<AgentRow[]>([]);
  const [agentTotal, setAgentTotal] = useState(0);
  const [agentPage, setAgentPage] = useState(1);
  const [agentLoading, setAgentLoading] = useState(false);
  const [pendingInvites, setPendingInvites] = useState<PendingInvite[]>([]);

  const [commissions, setCommissions] = useState<CommissionLog[]>([]);
  const [commTotal, setCommTotal] = useState(0);
  const [commPage, setCommPage] = useState(1);
  const [commLoading, setCommLoading] = useState(false);

  const [myRelation, setMyRelation] = useState<MyRelation | null>(null);
  const [myRelLoading, setMyRelLoading] = useState(false);
  const [myCommissions, setMyCommissions] = useState<{id:string;commission_rate:number;units:number;created_at:string}[]>([]);
  const [myTotalUnits, setMyTotalUnits] = useState(0);

  const [showInviteModal, setShowInviteModal] = useState(false);
  const [showQuotaModal, setShowQuotaModal] = useState<AgentRow | null>(null);
  const [showJoinModal, setShowJoinModal] = useState(false);
  const [inviteForm, setInviteForm] = useState({ quota_total: 100, commission_rate: 10, note: '' });
  const [quotaDelta, setQuotaDelta] = useState(50);
  const [joinCode, setJoinCode] = useState('');
  const [submitting, setSubmitting] = useState(false);

  const confirm = useConfirm();
  const PAGE_SIZE = 10;

  const loadAgents = (p = agentPage) => {
    setAgentLoading(true); setAgents([]);
    agentApi.listAgents({ page: p, page_size: PAGE_SIZE })
      .then(r => {
        if (r.data.success) {
          setAgents(r.data.data);
          setAgentTotal(r.data.total);
          setPendingInvites(r.data.pending_invites || []);
        }
      })
      .catch(e => { if (e?.name !== 'CanceledError' && e?.code !== 'ERR_CANCELED') throw e; })
      .finally(() => setAgentLoading(false));
  };

  const loadCommissions = (p = commPage) => {
    setCommLoading(true);
    agentApi.listCommissions({ page: p, page_size: PAGE_SIZE })
      .then(r => {
        if (r.data.success) { setCommissions(r.data.data); setCommTotal(r.data.total); }
      })
      .catch(e => {
        if (e?.name === 'CanceledError' || e?.code === 'ERR_CANCELED') {
          // 被去重取消，延迟重试一次
          setTimeout(() => {
            agentApi.listCommissions({ page: p, page_size: PAGE_SIZE })
              .then(r => { if (r.data.success) { setCommissions(r.data.data); setCommTotal(r.data.total); } })
              .finally(() => setCommLoading(false));
          }, 100);
          return;
        }
      })
      .finally(() => setCommLoading(false));
  };

  const loadMyRelation = () => {
    setMyRelLoading(true);
    const doLoad = () => Promise.all([
      agentApi.myRelation(),
      agentApi.myCommissions({ page: 1, page_size: PAGE_SIZE }),
    ]).then(([relRes, commRes]) => {
      if (relRes.data.success) setMyRelation(relRes.data.data);
      if (commRes.data.success) {
        setMyCommissions(commRes.data.data);
        setMyTotalUnits(commRes.data.total_units);
      }
    });
    doLoad()
      .catch(e => {
        if (e?.name === 'CanceledError' || e?.code === 'ERR_CANCELED') {
          setTimeout(() => doLoad().finally(() => setMyRelLoading(false)), 100);
          return;
        }
      })
      .finally(() => setMyRelLoading(false));
  };

  useEffect(() => {
    if (tab === 'my_agents') loadAgents(1);
    else if (tab === 'commissions') loadCommissions(1);
    else loadMyRelation();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  const handleCreateInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    try {
      const r = await agentApi.createInvite(inviteForm);
      if (r.data.success) {
        toast.success('邀请码已生成');
        navigator.clipboard.writeText(r.data.data.invite_code).catch(() => {});
        toast('邀请码已复制到剪贴板', { icon: '📋' });
        setShowInviteModal(false);
        setInviteForm({ quota_total: 100, commission_rate: 10, note: '' });
        loadAgents(1);
      } else toast.error(r.data.message);
    } catch { toast.error('创建失败'); }
    finally { setSubmitting(false); }
  };

  const handleUpdateQuota = async () => {
    if (!showQuotaModal) return;
    setSubmitting(true);
    try {
      const r = await agentApi.updateQuota(showQuotaModal.id, quotaDelta);
      if (r.data.success) { toast.success(r.data.message); setShowQuotaModal(null); loadAgents(); }
      else toast.error(r.data.message);
    } catch { toast.error('操作失败'); }
    finally { setSubmitting(false); }
  };

  const handleToggleStatus = async (agent: AgentRow) => {
    const newStatus = agent.status === 'active' ? 'disabled' : 'active';
    const ok = await confirm({
      title: newStatus === 'disabled' ? '禁用代理' : '启用代理',
      message: `确认${newStatus === 'disabled' ? '禁用' : '启用'}代理 ${agent.agent_username}？`,
      confirmText: newStatus === 'disabled' ? '禁用' : '启用',
      danger: newStatus === 'disabled',
    });
    if (!ok) return;
    try {
      const r = await agentApi.updateStatus(agent.id, newStatus);
      if (r.data.success) { toast.success('状态已更新'); loadAgents(); }
      else toast.error(r.data.message);
    } catch { toast.error('操作失败'); }
  };

  const handleRemove = async (agent: AgentRow) => {
    const ok = await confirm({
      title: '解除代理关系',
      message: `确认解除与 ${agent.agent_username} 的代理关系？此操作不可撤销。`,
      confirmText: '解除',
      danger: true,
    });
    if (!ok) return;
    try {
      const r = await agentApi.removeAgent(agent.id);
      if (r.data.success) { toast.success('已解除'); loadAgents(); }
      else toast.error(r.data.message);
    } catch { toast.error('操作失败'); }
  };

  const handleJoin = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    try {
      const r = await agentApi.joinByInvite(joinCode.trim().toUpperCase());
      if (r.data.success) {
        toast.success('已成功加入代理关系');
        setShowJoinModal(false); setJoinCode('');
        loadMyRelation(); setTab('my_relation');
      } else toast.error(r.data.message);
    } catch { toast.error('加入失败'); }
    finally { setSubmitting(false); }
  };

  const copyCode = (code: string) => {
    navigator.clipboard.writeText(code).catch(() => {});
    toast.success('邀请码已复制');
  };

  const agentPages = Math.ceil(agentTotal / PAGE_SIZE);
  const commPages  = Math.ceil(commTotal  / PAGE_SIZE);

  return (
    <div className="fade-in">
      {/* 页头 */}
      <div className="page-header">
        <div>
          <h1 className="page-title">代理管理</h1>
          <p className="page-subtitle">管理下级代理配额与分润，或加入上级代理关系</p>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => setShowJoinModal(true)}>
            <Network size={14} /> 加入代理关系
          </button>
          <button className="btn btn-primary" onClick={() => setShowInviteModal(true)}>
            <Plus size={14} /> 生成邀请码
          </button>
        </div>
      </div>

      {/* Tab */}
      <div style={{ display: 'flex', gap: 4, marginBottom: 20, borderBottom: '1px solid var(--border)', paddingBottom: 0 }}>
        {([['my_agents','我的代理',<Users size={14}/>],['commissions','分润统计',<TrendingUp size={14}/>],['my_relation','我的上级',<Network size={14}/>]] as const).map(([key, label, icon]) => (
          <button key={key} onClick={() => setTab(key)} style={{
            padding: '8px 16px', border: 'none', cursor: 'pointer', fontSize: 13,
            borderBottom: tab === key ? '2px solid var(--accent)' : '2px solid transparent',
            color: tab === key ? 'var(--accent)' : 'var(--text-muted)',
            background: 'none', display: 'flex', alignItems: 'center', gap: 6,
          }}>
            {icon} {label}
          </button>
        ))}
      </div>

      {/* ── 我的代理列表 ── */}
      {tab === 'my_agents' && (
        <>
          {pendingInvites.length > 0 && (
            <div style={{ marginBottom: 16, padding: '12px 16px', background: 'rgba(251,191,36,0.08)', border: '1px solid rgba(251,191,36,0.25)', borderRadius: 10 }}>
              <p style={{ fontSize: 12, fontWeight: 600, color: '#fbbf24', marginBottom: 8 }}>待使用邀请码</p>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                {pendingInvites.map(inv => (
                  <button key={inv.invite_code} onClick={() => copyCode(inv.invite_code)}
                    className="btn btn-ghost"
                    style={{ fontFamily: 'monospace', fontSize: 13, gap: 6 }}>
                    {inv.invite_code} <Copy size={12}/>
                    <span style={{ color: 'var(--text-muted)', fontSize: 11 }}>配额{inv.quota_total} · 分润{inv.commission_rate}%</span>
                  </button>
                ))}
              </div>
            </div>
          )}
          <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: 12 }}>
            <button className="btn btn-ghost" onClick={() => loadAgents()}><RefreshCw size={14}/> 刷新</button>
          </div>
          <div className="table-wrap">
            <table>
              <thead><tr><th>代理用户</th><th>已用 / 配额</th><th>分润比例</th><th>状态</th><th>邀请码</th><th>加入时间</th><th>操作</th></tr></thead>
              <tbody>
                {agentLoading ? Array.from({length:5}).map((_,i) => (
                  <tr key={i} className="skeleton-row">
                    {Array.from({length:7}).map((_,j) => <td key={j}><span className="skeleton" style={{width:'60%'}}/></td>)}
                  </tr>
                )) : agents.length === 0 ? (
                  <tr><td colSpan={7}><div className="empty-state"><div className="empty-state-icon">👥</div><div className="empty-state-text">暂无代理，生成邀请码邀请代理加入</div></div></td></tr>
                ) : agents.map((a, idx) => (
                  <tr key={a.id} className="data-enter" style={{animationDelay:`${idx*30}ms`}}>
                    <td><span style={{fontWeight:600}}>{a.agent_username}</span></td>
                    <td>
                      <div style={{display:'flex',alignItems:'center',gap:8}}>
                        <span className="mono" style={{fontSize:12}}>{a.quota_used} / {a.quota_total}</span>
                        <div style={{width:60,height:4,background:'var(--border)',borderRadius:2,overflow:'hidden'}}>
                          <div style={{height:'100%',background:'var(--accent)',borderRadius:2,width:`${a.quota_total>0?Math.min(100,a.quota_used/a.quota_total*100):0}%`}}/>
                        </div>
                      </div>
                    </td>
                    <td><span style={{color:'var(--accent)',fontWeight:600}}>{a.commission_rate}%</span></td>
                    <td>
                      <span style={{
                        padding:'2px 8px',borderRadius:12,fontSize:12,
                        background: a.status==='active' ? 'rgba(52,211,153,0.15)' : 'rgba(248,113,113,0.15)',
                        color: a.status==='active' ? '#34d399' : '#f87171',
                      }}>{a.status==='active'?'正常':'禁用'}</span>
                    </td>
                    <td>
                      <button className="btn btn-ghost" style={{fontSize:12,fontFamily:'monospace',gap:4}} onClick={() => copyCode(a.invite_code)}>
                        {a.invite_code} <Copy size={11}/>
                      </button>
                    </td>
                    <td><span style={{fontSize:12}}>{new Date(a.created_at).toLocaleDateString('zh-CN')}</span></td>
                    <td>
                      <div style={{display:'flex',gap:6}}>
                        <button className="btn btn-sm btn-ghost" onClick={() => {setShowQuotaModal(a);setQuotaDelta(50);}}>调配额</button>
                        <button className={`btn btn-sm ${a.status==='active'?'btn-ghost':'btn-ghost'}`}
                          style={{color: a.status==='active'?'#fb923c':'#34d399'}}
                          onClick={() => handleToggleStatus(a)}>{a.status==='active'?'禁用':'启用'}</button>
                        <button className="btn btn-sm btn-danger" onClick={() => handleRemove(a)}><Trash2 size={12}/></button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {agentPages > 1 && (
            <div className="pagination">
              {Array.from({length:agentPages},(_,i)=>i+1).map(p => (
                <button key={p} className={`page-btn ${p===agentPage?'active':''}`}
                  onClick={() => {setAgentPage(p);loadAgents(p);}}>{p}</button>
              ))}
            </div>
          )}
        </>
      )}

      {/* ── 分润统计 ── */}
      {tab === 'commissions' && (
        <>
          <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: 12 }}>
            <button className="btn btn-ghost" onClick={() => loadCommissions()}><RefreshCw size={14}/> 刷新</button>
          </div>
          <div className="table-wrap">
            <table>
              <thead><tr><th>代理用户</th><th>分润比例</th><th>激活数</th><th>时间</th></tr></thead>
              <tbody>
                {commLoading ? Array.from({length:5}).map((_,i) => (
                  <tr key={i} className="skeleton-row">
                    {Array.from({length:4}).map((_,j) => <td key={j}><span className="skeleton" style={{width:'60%'}}/></td>)}
                  </tr>
                )) : commissions.length === 0 ? (
                  <tr><td colSpan={4}><div className="empty-state"><div className="empty-state-icon">📊</div><div className="empty-state-text">暂无分润记录</div></div></td></tr>
                ) : commissions.map((c, idx) => (
                  <tr key={c.id} className="data-enter" style={{animationDelay:`${idx*30}ms`}}>
                    <td><span style={{fontWeight:600}}>{c.agent_username}</span></td>
                    <td><span style={{color:'var(--accent)',fontWeight:600}}>{c.commission_rate}%</span></td>
                    <td><span className="mono">{c.units}</span></td>
                    <td><span style={{fontSize:12}}>{new Date(c.created_at).toLocaleString('zh-CN')}</span></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {commPages > 1 && (
            <div className="pagination">
              {Array.from({length:commPages},(_,i)=>i+1).map(p => (
                <button key={p} className={`page-btn ${p===commPage?'active':''}`}
                  onClick={() => {setCommPage(p);loadCommissions(p);}}>{p}</button>
              ))}
            </div>
          )}
        </>
      )}

      {/* ── 我的上级 ── */}
      {tab === 'my_relation' && (
        <>
          {myRelLoading ? (
            <div style={{padding:'48px 0',textAlign:'center',color:'var(--text-muted)'}}>加载中…</div>
          ) : myRelation ? (
            <>
              <div style={{display:'grid',gridTemplateColumns:'repeat(4,1fr)',gap:12,marginBottom:20}}>
                {([['上级商户', myRelation.parent_username, ''],
                  ['已用 / 配额', `${myRelation.quota_used} / ${myRelation.quota_total}`, ''],
                  ['分润比例', `${myRelation.commission_rate}%`, 'var(--accent)'],
                  ['状态', myRelation.status==='active'?'正常':'已禁用', myRelation.status==='active'?'#34d399':'#f87171'],
                ] as [string,string,string][]).map(([label, val, color]) => (
                  <div key={label} className="card" style={{padding:'16px 20px'}}>
                    <p style={{fontSize:11,color:'var(--text-muted)',marginBottom:6}}>{label}</p>
                    <p style={{fontSize:16,fontWeight:700,color:color||'var(--text)'}}>{val}</p>
                  </div>
                ))}
              </div>
              <div style={{display:'flex',alignItems:'center',gap:8,marginBottom:16,padding:'10px 16px',background:'rgba(124,106,247,0.08)',borderRadius:8,border:'1px solid rgba(124,106,247,0.2)'}}>
                <TrendingUp size={15} style={{color:'var(--accent)'}}/>
                <span style={{fontSize:13,color:'var(--text-dim)'}}>我的累计激活：<span style={{fontWeight:700,color:'var(--accent)'}}>{myTotalUnits} 张</span></span>
              </div>
              <div className="table-wrap">
                <table>
                  <thead><tr><th>分润比例</th><th>激活数</th><th>时间</th></tr></thead>
                  <tbody>
                    {myCommissions.length === 0 ? (
                      <tr><td colSpan={3}><div className="empty-state"><div className="empty-state-icon">📋</div><div className="empty-state-text">暂无激活记录</div></div></td></tr>
                    ) : myCommissions.map((c,idx) => (
                      <tr key={c.id} className="data-enter" style={{animationDelay:`${idx*30}ms`}}>
                        <td><span style={{color:'var(--accent)',fontWeight:600}}>{c.commission_rate}%</span></td>
                        <td><span className="mono">{c.units}</span></td>
                        <td><span style={{fontSize:12}}>{new Date(c.created_at).toLocaleString('zh-CN')}</span></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          ) : (
            <div className="empty-state" style={{padding:'64px 0'}}>
              <div className="empty-state-icon">🔗</div>
              <div className="empty-state-text">您尚未加入任何代理关系</div>
              <button className="btn btn-primary" style={{marginTop:16}} onClick={() => setShowJoinModal(true)}>使用邀请码加入</button>
            </div>
          )}
        </>
      )}

      {/* ── 生成邀请码弹窗 ── */}
      {showInviteModal && (
        <div className="modal-overlay" onClick={() => setShowInviteModal(false)}>
          <div className="modal" style={{maxWidth:420}} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">生成邀请码</h2>
            <form onSubmit={handleCreateInvite}>
              <div className="form-group">
                <label className="form-label">初始配额</label>
                <input type="number" min={0} value={inviteForm.quota_total}
                  onChange={e => setInviteForm(f => ({...f, quota_total: +e.target.value}))} />
              </div>
              <div className="form-group">
                <label className="form-label">分润比例（%）</label>
                <input type="number" min={0} max={100} value={inviteForm.commission_rate}
                  onChange={e => setInviteForm(f => ({...f, commission_rate: +e.target.value}))} />
              </div>
              <div className="form-group">
                <label className="form-label">备注（可选）</label>
                <input type="text" value={inviteForm.note}
                  onChange={e => setInviteForm(f => ({...f, note: e.target.value}))} />
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => setShowInviteModal(false)}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting}>
                  {submitting ? <span className="spinner" /> : '生成并复制'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* ── 调整配额弹窗 ── */}
      {showQuotaModal && (
        <div className="modal-overlay" onClick={() => setShowQuotaModal(null)}>
          <div className="modal" style={{maxWidth:380}} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">调整配额</h2>
            <p style={{fontSize:13,color:'var(--text-muted)',marginBottom:16}}>
              代理：<strong>{showQuotaModal.agent_username}</strong>，当前配额 {showQuotaModal.quota_total}（已用 {showQuotaModal.quota_used}）
            </p>
            <div className="form-group">
              <label className="form-label">变更量（正数增加 / 负数回收）</label>
              <input type="number" value={quotaDelta} onChange={e => setQuotaDelta(+e.target.value)} />
              <span style={{fontSize:11,color:'var(--text-muted)'}}>调整后配额：{showQuotaModal.quota_total + quotaDelta}</span>
            </div>
            <div className="modal-actions">
              <button className="btn btn-ghost" onClick={() => setShowQuotaModal(null)}>取消</button>
              <button className="btn btn-primary" disabled={submitting} onClick={handleUpdateQuota}>
                {submitting ? <span className="spinner" /> : '确认调整'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ── 加入代理弹窗 ── */}
      {showJoinModal && (
        <div className="modal-overlay" onClick={() => setShowJoinModal(false)}>
          <div className="modal" style={{maxWidth:380}} onClick={e => e.stopPropagation()}>
            <h2 className="modal-title">使用邀请码加入</h2>
            <form onSubmit={handleJoin}>
              <div className="form-group">
                <label className="form-label">邀请码</label>
                <input type="text" value={joinCode} onChange={e => setJoinCode(e.target.value)}
                  placeholder="请输入 8 位邀请码"
                  style={{fontFamily:'monospace',textTransform:'uppercase',letterSpacing:2}} />
              </div>
              <div className="modal-actions">
                <button type="button" className="btn btn-ghost" onClick={() => setShowJoinModal(false)}>取消</button>
                <button type="submit" className="btn btn-primary" disabled={submitting || !joinCode.trim()}>
                  {submitting ? <span className="spinner" /> : '确认加入'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}

