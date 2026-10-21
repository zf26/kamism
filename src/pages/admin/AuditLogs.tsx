import { useEffect, useState } from 'react';
import { adminApi } from '../../lib/api';
import { RefreshCw, Search, ScrollText, ShieldAlert } from 'lucide-react';

interface AuditLog {
  id: number;
  admin_id: string | null;
  admin_email: string | null;
  method: string;
  route: string;
  path: string;
  status: number;
  detail: Record<string, unknown> | null;
  ip: string | null;
  created_at: string;
}

const METHOD_COLOR: Record<string, string> = {
  POST: '#22c55e',
  PUT: '#f59e0b',
  PATCH: '#a78bfa',
  DELETE: '#ef4444',
};

function statusColor(status: number) {
  if (status >= 500) return '#ef4444';
  if (status >= 400) return '#f59e0b';
  return '#22c55e';
}

/**
 * 管理端操作日志。
 *
 * 数据由后端 `middleware::audit` 自动写入 —— 所有管理端**写操作**
 * （POST/PUT/PATCH/DELETE）都会留痕，包括失败的那些。
 * 这里只读，不提供删除入口：审计日志能改就等于没有。
 */
export default function AuditLogs() {
  const [logs, setLogs] = useState<AuditLog[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [keyword, setKeyword] = useState('');
  const [applied, setApplied] = useState('');
  const [loading, setLoading] = useState(true);
  const pageSize = 20;

  const load = () => {
    setLoading(true);
    adminApi.getAuditLogs({ page, page_size: pageSize, keyword: applied || undefined })
      .then(res => {
        if (res.data.success) {
          setLogs(res.data.data ?? []);
          setTotal(res.data.total ?? 0);
        }
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  };

  useEffect(() => { load(); }, [page, applied]);

  const totalPages = Math.max(1, Math.ceil(total / pageSize));

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    setPage(1);
    setApplied(keyword.trim());
  };

  return (
    <div className="fade-in">
      <div className="page-header">
        <div>
          <h1 className="page-title">操作日志</h1>
          <p className="page-subtitle">
            {loading ? '加载中…' : `共 ${total} 条记录 · 管理端写操作自动留痕`}
          </p>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <form onSubmit={handleSearch} style={{ display: 'flex', gap: 6 }}>
            <div style={{ position: 'relative' }}>
              <Search size={14} style={{ position: 'absolute', left: 10, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)' }} />
              <input
                value={keyword}
                onChange={e => setKeyword(e.target.value)}
                placeholder="搜索动作 / 路径"
                style={{ paddingLeft: 32, width: 220 }}
              />
            </div>
            <button type="submit" className="btn btn-ghost"><Search size={14} /></button>
          </form>
          <button className="btn btn-ghost" onClick={load}><RefreshCw size={14} /> 刷新</button>
        </div>
      </div>

      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th style={{ width: 150 }}>时间</th>
              <th style={{ width: 170 }}>操作人</th>
              <th style={{ width: 90 }}>动作</th>
              <th>对象</th>
              <th style={{ width: 130 }}>变更内容</th>
              <th style={{ width: 70 }}>结果</th>
              <th style={{ width: 120 }}>来源 IP</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              [...Array(5)].map((_, i) => (
                <tr key={i}>
                  {[...Array(7)].map((__, j) => (
                    <td key={j}><span className="skeleton" style={{ display: 'block', height: 14, borderRadius: 4 }} /></td>
                  ))}
                </tr>
              ))
            ) : logs.length === 0 ? (
              <tr>
                <td colSpan={7}>
                  <div style={{ textAlign: 'center', padding: '48px 0', color: 'var(--text-muted)' }}>
                    <ScrollText size={28} style={{ opacity: 0.4, marginBottom: 10 }} />
                    <p>暂无操作记录</p>
                    <p style={{ fontSize: 12, marginTop: 6 }}>
                      管理端的每一次写操作（改套餐、禁用、删商户、改配置）都会记录在这里
                    </p>
                  </div>
                </td>
              </tr>
            ) : (
              logs.map(log => (
                <tr key={log.id}>
                  <td className="mono" style={{ fontSize: 12, color: 'var(--text-muted)' }}>
                    {new Date(log.created_at).toLocaleString('zh-CN')}
                  </td>
                  <td style={{ fontSize: 12 }}>
                    {log.admin_email ?? <span style={{ color: 'var(--text-muted)' }}>（已删除的账号）</span>}
                  </td>
                  <td>
                    <span
                      className="mono"
                      style={{
                        fontSize: 11, fontWeight: 700,
                        color: METHOD_COLOR[log.method] ?? 'var(--text-muted)',
                      }}
                    >
                      {log.method}
                    </span>
                  </td>
                  <td>
                    <div className="mono" style={{ fontSize: 12, color: 'var(--text)' }}>{log.route}</div>
                    {log.path !== log.route && (
                      <div className="mono" style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 2, wordBreak: 'break-all' }}>
                        {log.path}
                      </div>
                    )}
                  </td>
                  <td style={{ fontSize: 11, color: 'var(--text-muted)', wordBreak: 'break-all' }}>
                    {log.detail
                      ? Object.entries(log.detail).map(([k, v]) => `${k}=${String(v)}`).join(', ')
                      : '—'}
                  </td>
                  <td>
                    <span
                      style={{
                        fontSize: 12, fontWeight: 700,
                        color: statusColor(log.status),
                      }}
                    >
                      {log.status}
                    </span>
                  </td>
                  <td className="mono" style={{ fontSize: 11, color: 'var(--text-muted)' }}>{log.ip ?? '—'}</td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      {totalPages > 1 && (
        <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', gap: 10, marginTop: 20 }}>
          <button className="btn btn-ghost btn-sm" disabled={page <= 1} onClick={() => setPage(p => p - 1)}>上一页</button>
          <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>{page} / {totalPages}</span>
          <button className="btn btn-ghost btn-sm" disabled={page >= totalPages} onClick={() => setPage(p => p + 1)}>下一页</button>
        </div>
      )}

      <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 16, display: 'flex', alignItems: 'center', gap: 6 }}>
        <ShieldAlert size={13} />
        日志只追加、不提供修改或删除接口；其中只记录运营字段（套餐、天数、状态等），不记录任何密钥。
      </p>
    </div>
  );
}
