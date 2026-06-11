import { useEffect, useState, useRef } from 'react';
import { exeShieldApi, appsApi } from '../../lib/api';
import { useAuthStore } from '../../stores/auth';
import { Upload, RefreshCw, Download, Clock, CheckCircle2, XCircle, Loader2, Package, Monitor, Heart } from 'lucide-react';
import toast from 'react-hot-toast';

interface App {
  id: string;
  app_name: string;
  description: string | null;
  status: string;
  created_at: string;
}

interface Job {
  job_id: string;
  app_id: string;
  status: string;
  protected_url: string | null;
  error: string | null;
  created_at: string;
  app_name?: string;
}

const STATUS_CONFIG: Record<string, { label: string; color: string; bg: string; icon: React.ReactNode }> = {
  pending:    { label: '排队中', color: '#f59e0b', bg: 'rgba(245,158,11,0.1)',  icon: <Clock size={12} /> },
  processing: { label: '加固中', color: '#7c6af7', bg: 'rgba(124,106,247,0.1)', icon: <Loader2 size={12} className="spin" /> },
  done:       { label: '已完成', color: '#10b981', bg: 'rgba(16,185,129,0.1)',  icon: <CheckCircle2 size={12} /> },
  failed:     { label: '失败',   color: '#ef4444', bg: 'rgba(239,68,68,0.1)',  icon: <XCircle size={12} /> },
};

const PAGE_SIZE_OPTIONS = [5, 10, 15, 20];

export default function ExeShield() {
  const { user } = useAuthStore();
  const [apps, setApps] = useState<App[]>([]);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [loading, setLoading] = useState(true);
  const [uploading, setUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState<string>('');
  const [healthLoading, setHealthLoading] = useState(false);
  const [healthStatus, setHealthStatus] = useState<'ok' | 'error' | null>(null);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [total, setTotal] = useState(0);

  // 上传表单
  const [selectedAppId, setSelectedAppId] = useState('');
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [windowTitle] = useState('');
  const [windowHint] = useState('');
  const fileInputRef = useRef<HTMLInputElement>(null);

  // 加载应用列表（用于下拉选择）
  const loadApps = () => {
    appsApi.list({ page: 1, page_size: 100 }).then(res => {
      if (res.data.success) setApps(res.data.data ?? []);
    }).catch(() => {});
  };

  const loadJobs = () => {
    const merchantId = user?.id;
    if (!merchantId) return;
    setLoading(true);
    exeShieldApi.list(merchantId, page, pageSize).then(res => {
      if (res.data.success) {
        const respData = res.data.data;
        const data: Job[] = Array.isArray(respData) ? respData : (respData?.data ?? []);
        const totalCount = typeof respData === 'object' && respData !== null
          ? (respData.total ?? data.length)
          : data.length;
        const appMap: Record<string, string> = {};
        apps.forEach(a => { appMap[a.id] = a.app_name; });
        setJobs(data.map(j => ({
          ...j,
          app_name: (j.app_name && j.app_name.trim()) ? j.app_name : (appMap[j.app_id ?? ''] ?? undefined),
        })));
        setTotal(totalCount);
      }
    }).catch(() => {}).finally(() => setLoading(false));
  };

  // 健康检查
  const handleHealthCheck = async () => {
    setHealthLoading(true);
    setHealthStatus(null);
    try {
      await exeShieldApi.health();
      setHealthStatus('ok');
      toast.success('EXE 加固服务运行正常');
    } catch {
      setHealthStatus('error');
      toast.error('EXE 加固服务不可用');
    } finally {
      setHealthLoading(false);
    }
  };

  useEffect(() => { loadApps(); }, []);
  useEffect(() => {
    if (apps.length > 0) loadJobs();
  }, [apps, page, pageSize]);

  // 上传 EXE
  const handleUpload = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedAppId) { toast.error('请选择应用'); return; }
    if (!selectedFile) { toast.error('请选择 EXE 文件'); return; }
    if (!selectedFile.name.toLowerCase().endsWith('.exe')) { toast.error('只支持 .exe 文件'); return; }
    const apiKey = user?.api_key;
    if (!apiKey) { toast.error('无法获取 API Key，请重新登录'); return; }

    const merchantId = user?.id;
    if (!merchantId) { toast.error('无法获取商户信息，请重新登录'); return; }

    setUploading(true);
    setUploadProgress('正在上传...');

    try {
      await exeShieldApi.upload(
        selectedFile,
        selectedAppId,
        merchantId,
        apiKey,
        selectedFile.name,
        windowTitle.trim() || undefined,
        windowHint.trim() || undefined,
      );
      toast.success('上传成功，加固任务已创建');
      setSelectedFile(null);
      if (fileInputRef.current) fileInputRef.current.value = '';
      setPage(1);
      setTimeout(() => loadJobs(), 500);
    } catch (err: any) {
      const msg = err?.response?.data?.message ?? err?.message ?? '上传失败';
      toast.error(msg);
    } finally {
      setUploading(false);
      setUploadProgress('');
    }
  };

  // 轮询进行中的任务
  useEffect(() => {
    const pending = jobs.filter(j => j.status === 'pending' || j.status === 'processing');
    if (pending.length === 0) return;

    const interval = setInterval(() => {
      pending.forEach(job => {
        exeShieldApi.status(job.job_id).then(res => {
          if (res.data.success) {
            setJobs(prev => prev.map(j =>
              j.job_id === job.job_id
                ? { ...j, ...res.data.data, app_name: j.app_name }
                : j
            ));
          }
        }).catch(() => {});
      });
    }, 3000);

    return () => clearInterval(interval);
  }, [jobs]);

  // 下载
  const handleDownload = async (job: Job) => {
    try {
      const res = await exeShieldApi.download(job.job_id);
      const blob = res.data;
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${job.job_id}_protected.exe`;
      a.click();
      URL.revokeObjectURL(url);
      toast.success('下载成功');
    } catch (err: any) {
      toast.error(err?.response?.data?.message ?? '下载失败');
    }
  };

  const handlePageSize = (ps: number) => { setPage(1); setPageSize(ps); };

  const now = Date.now();

  return (
    <div className="fade-in">
      {/* 页面头部 */}
      <div className="page-header">
        <div>
          <h1 className="page-title">EXE 加固<span className="text-muted">(Beta)</span></h1>
          <p className="page-subtitle">上传 EXE，自动注入卡密验证壳，下载加固后程序</p>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <button
            className={`btn btn-ghost ${healthStatus === 'ok' ? 'btn-success-ghost' : healthStatus === 'error' ? 'btn-danger-ghost' : ''}`}
            style={{ display: 'flex', alignItems: 'center', gap: 6 }}
            onClick={handleHealthCheck}
            disabled={healthLoading}
          >
            {healthLoading
              ? <Loader2 size={14} className="spin" />
              : <Heart size={14} />}
            服务健康
          </button>
          <button className="btn btn-ghost" onClick={() => loadJobs()}>
            <RefreshCw size={14} /> 刷新
          </button>
        </div>
      </div>

      {/* 上传区域 */}
      <div className="card" style={{ marginBottom: 20 }}>
        <p style={{ fontWeight: 700, marginBottom: 16, color: 'var(--text)', display: 'flex', alignItems: 'center', gap: 6 }}>
          <Upload size={15} /> 上传 EXE
        </p>
        <form onSubmit={handleUpload}>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 16 }}>
            <div className="form-group" style={{ margin: 0 }}>
              <label className="form-label">目标应用 *</label>
              <select
                className="input"
                value={selectedAppId}
                onChange={e => setSelectedAppId(e.target.value)}
                required
              >
                <option value="">— 选择应用 —</option>
                {apps.filter(a => a.status === 'active').map(a => (
                  <option key={a.id} value={a.id}>{a.app_name}</option>
                ))}
              </select>
              <p style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 4 }}>
                {apps.length === 0 ? '暂无应用，请先在「我的应用」中创建' : ''}
              </p>
            </div>
            <div className="form-group" style={{ margin: 0 }}>
              <label className="form-label">EXE 文件 *</label>
              <input
                ref={fileInputRef}
                type="file"
                accept=".exe"
                className="input"
                onChange={e => setSelectedFile(e.target.files?.[0] ?? null)}
                required
              />
              {selectedFile && (
                <p style={{ fontSize: 11, color: 'var(--accent)', marginTop: 4 }}>
                  已选: {selectedFile.name} ({(selectedFile.size / 1024 / 1024).toFixed(1)} MB)
                </p>
              )}
            </div>
          </div>
          <div style={{ marginTop: 16, display: 'flex', alignItems: 'center', gap: 12 }}>
            <button type="submit" className="btn btn-primary" disabled={uploading || apps.length === 0}>
              {uploading ? (
                <><Loader2 size={14} className="spin" /> {uploadProgress || '上传中...'}</>
              ) : (
                <><Upload size={14} /> 开始加固</>
              )}
            </button>
            <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
              加固仅支持在本地运行，请不要将加固后的 EXE 上传到任何网站或服务器
            </span>
          </div>
        </form>
      </div>

      {/* 任务列表 */}
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>任务ID</th>
              <th>应用</th>
              <th>状态</th>
              <th>创建时间</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              Array.from({ length: pageSize }).map((_, i) => (
                <tr key={i} className="skeleton-row">
                  <td><span className="skeleton" style={{ width: '70%' }} /></td>
                  <td><span className="skeleton" style={{ width: '60%' }} /></td>
                  <td><span className="skeleton" style={{ width: '48px' }} /></td>
                  <td><span className="skeleton" style={{ width: '55%' }} /></td>
                  <td><span className="skeleton" style={{ width: '64px', height: '28px' }} /></td>
                </tr>
              ))
            ) : jobs.length === 0 ? (
              <tr>
                <td colSpan={5}>
                  <div className="empty-state">
                    <div className="empty-state-icon"><Monitor size={32} /></div>
                    <div className="empty-state-text">暂无加固记录，上传第一个 EXE 开始吧</div>
                  </div>
                </td>
              </tr>
            ) : jobs.map((job, idx) => {
              const cfg = STATUS_CONFIG[job.status] ?? STATUS_CONFIG['pending'];
              const age = Math.round((now - new Date(job.created_at).getTime()) / 1000);
              return (
                <tr key={job.job_id} className="data-enter" style={{ animationDelay: `${idx * 30}ms` }}>
                  <td>
                    <span
                      className="mono"
                      style={{ fontSize: 11, cursor: 'pointer', color: 'var(--text-muted)', display: 'inline-flex', alignItems: 'center', gap: 4 }}
                      title="点击复制任务ID"
                      onClick={() => { navigator.clipboard.writeText(job.job_id); toast.success('任务ID已复制'); }}
                    >
                      {job.job_id}<Package size={10} />
                    </span>
                  </td>
                  <td>
                    <span style={{ color: 'var(--text)', fontWeight: 600 }}>
                      {job.app_name ?? <span style={{ color: 'var(--text-muted)', fontStyle: 'italic' }}>未知应用</span>}
                    </span>
                  </td>
                  <td>
                    <span
                      style={{
                        display: 'inline-flex', alignItems: 'center', gap: 4,
                        fontSize: 12, fontWeight: 700,
                        color: cfg.color, background: cfg.bg,
                        padding: '3px 8px', borderRadius: 5,
                      }}
                    >
                      {cfg.icon}
                      {cfg.label}
                    </span>
                    {job.status === 'failed' && job.error && (
                      <div style={{ fontSize: 11, color: 'var(--danger)', marginTop: 4, maxWidth: 200 }}>
                        {job.error.length > 40 ? job.error.slice(0, 40) + '…' : job.error}
                      </div>
                    )}
                  </td>
                  <td>
                    <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
                      {new Date(job.created_at).toLocaleString('zh-CN')}
                      <span style={{ marginLeft: 6, fontSize: 11, opacity: 0.6 }}>
                        {age < 60 ? `${age}s前` : age < 3600 ? `${Math.floor(age / 60)}m前` : `${Math.floor(age / 3600)}h前`}
                      </span>
                    </span>
                  </td>
                  <td>
                    {job.status === 'done' ? (
                      <button
                        className="btn btn-sm btn-primary"
                        style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}
                        onClick={() => handleDownload(job)}
                      >
                        <Download size={12} /> 下载
                      </button>
                    ) : job.status === 'failed' ? (
                      <button
                        className="btn btn-sm btn-ghost"
                        style={{ color: 'var(--text-muted)' }}
                        onClick={() => toast.error(job.error ?? '加固失败，请重试')}
                      >
                        查看错误
                      </button>
                    ) : (
                      <span style={{ fontSize: 12, color: 'var(--text-muted)', display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                        <Loader2 size={12} className="spin" /> 处理中
                      </span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {/* 分页 */}
      <div className="pagination">
        <button className="page-btn" onClick={() => setPage(p => Math.max(1, p - 1))} disabled={page === 1}>‹</button>
        {Array.from({ length: Math.ceil(total / pageSize) }, (_, i) => i + 1)
          .slice(Math.max(0, page - 3), Math.min(Math.ceil(total / pageSize), page + 2))
          .map(p => (
            <button key={p} className={`page-btn ${p === page ? 'active' : ''}`} onClick={() => setPage(p)}>{p}</button>
          ))}
        <button className="page-btn" onClick={() => setPage(p => Math.min(Math.ceil(total / pageSize), p + 1))} disabled={page >= Math.ceil(total / pageSize)}>›</button>
        <span style={{ color: 'var(--text-muted)', fontSize: 12, margin: '0 4px' }}>每页</span>
        {PAGE_SIZE_OPTIONS.map(s => (
          <button key={s} className={`page-btn ${s === pageSize ? 'active' : ''}`} onClick={() => handlePageSize(s)}>{s}</button>
        ))}
      </div>
    </div>
  );
}
