import axios, { type InternalAxiosRequestConfig } from 'axios';
import type { AxiosInstance } from 'axios';

// API 地址从环境变量读取
const BASE_URL = import.meta.env.VITE_API_URL || 'http://localhost:9527';
const api = axios.create({
  baseURL: BASE_URL,
  timeout: 15000,
});
export { api };

// APK 加固服务（独立 base URL）
const SHIELD_URL = import.meta.env.VITE_SHIELD_URL || 'http://localhost:9529';
const shieldApi: AxiosInstance = axios.create({
  baseURL: SHIELD_URL,
  timeout: 600_000, // 10min，超大 APK 上传需要更长时间
});

// ── 请求去重：防止「同一接口的旧请求」覆盖「新请求」（翻页 / 搜索竞态）────────
//
// key = method + url（**不含 params**）。为什么不能含 params：
//   翻页时 `page=1` → `page=2`、搜索时 `card_code=a` → `card_code=ab`，params 不同则
//   key 不同，旧的慢请求不会被取消，会乱序覆盖新结果（列表页「搜到的还是上一页」）。
//   同一 url 的连续请求语义上是「同一份数据的最新查询」，旧的理应作废。
//
// 为什么不担心「同 url 不同资源」误伤：
//   - 详情/资源接口 url 自带 id（如 `/cards/:id`、`/activations/:id`），天然不同 key；
//   - 少数 `?order_id=x` 这类「同 url 不同参数」的 GET，前端都是串行调用（查完一个
//     再查下一个），不存在并发在途；即便极少数并发，被 abort 的也只是该作废的旧请求。
//
// value = AbortController，同时回挂到 config 的 `__controller` 上。
// 判断「这次响应/错误是否还属于当前在途请求」用 **controller 同一性**，而不是仅凭 key：
//   旧请求的回调可能在「新请求已经接管该 key」之后才回来，此时按 key 直接 delete 会
//   误删新请求的记录，让它失去被取消的能力（去重形同虚设，旧数据仍可能覆盖新数据）。
//
// 🚨 「被放弃的请求」必须**对调用方完全透明**（这是踩过的坑）：
//   放弃有两种来源 —— ① 被新请求 abort（错误回调收到 CanceledError）；
//   ② abort 没赶上，响应已在途（回来时 controller 已不是当前值）。
//   两者在响应拦截器里一律返回一个**永不 settle 的 promise**：调用方的 then / catch /
//   finally 全都不执行，等于这次请求从未发生。
//   反面教材：若在这里 `Promise.reject`，调用方的 `.catch()` 会把「旧请求被自己人
//   取消」误报成业务失败 —— 现象是**数据明明加载成功、却弹出「加载失败」**。开发模式
//   下 React.StrictMode 会让每个 useEffect 跑两次，第二次必然取消第一次，所以每次进
//   页面必现；生产环境翻页 / 搜索 / 防抖同样会命中。
//   loading 由接替它的新请求收尾（取消永远发生在「新请求即将发出」时，必有新请求收尾）。
const pendingRequests = new Map<string, AbortController>();

type TrackedRequestConfig = InternalAxiosRequestConfig & { __controller?: AbortController };

function buildRequestKey(config: InternalAxiosRequestConfig): string {
  return `${config.method?.toUpperCase()}:${config.url}`;
}

/** 请求是否被主动取消（AbortController.abort()）。覆盖 axios 与原生两种错误形态。 */
function isCanceledError(err: unknown): boolean {
  const e = err as { name?: string; code?: string } | undefined;
  return (
    axios.isCancel(err) ||
    e?.name === 'CanceledError' ||
    e?.name === 'AbortError' ||
    e?.code === 'ERR_CANCELED'
  );
}

/** 该 GET 请求是否已被更新的同 url 请求接管（abort 没赶上时的兜底判断）。 */
function isSuperseded(config: InternalAxiosRequestConfig | undefined): boolean {
  if (!config || config.method?.toUpperCase() !== 'GET') return false;
  const controller = (config as TrackedRequestConfig).__controller;
  return !!controller && pendingRequests.get(buildRequestKey(config)) !== controller;
}

/** 永不 settle 的 promise：用于「静默作废」一次请求，调用方无感。 */
function neverSettle(): Promise<never> {
  return new Promise<never>(() => {});
}

// 保留函数签名兼容性，无需异步初始化
export async function initApiUrl() {
  // 地址已在构建时确定，无需运行时初始化
}

// 是否正在刷新 token，防止并发请求重复刷新
let isRefreshing = false;
// 刷新期间等待的请求队列
let refreshQueue: Array<(token: string) => void> = [];

const flushQueue = (token: string) => {
  refreshQueue.forEach(cb => cb(token));
  refreshQueue = [];
};

const flushQueueReject = () => {
  refreshQueue.forEach(cb => cb(''));
  refreshQueue = [];
};

// ── 请求拦截器 ①：GET 去重 ──────────────────────────────────────────────────
api.interceptors.request.use((config) => {
  if (config.method?.toUpperCase() === 'GET') {
    const key = buildRequestKey(config);
    const existing = pendingRequests.get(key);
    if (existing) {
      // 取消上一个相同请求（它对应的错误回调会静默作废，不会打扰调用方）
      existing.abort();
    }
    const controller = new AbortController();
    config.signal = controller.signal;
    (config as TrackedRequestConfig).__controller = controller;
    // 直接覆盖：新请求即刻接管该 key
    pendingRequests.set(key, controller);
  }
  return config;
}, (error) => Promise.reject(error));

// ── 请求拦截器 ②：自动携带 token ────────────────────────────────────────────
api.interceptors.request.use((config) => {
  const token = localStorage.getItem('token');
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

// ── 响应拦截器：清除 pending 记录 + 401 自动续期 ─────────────────────────────
api.interceptors.response.use(
  (res) => {
    // 已被更新的同 url 请求接管 → 这次结果作废，静默丢弃（不打扰调用方）
    if (isSuperseded(res.config)) {
      return neverSettle();
    }
    // 请求完成，从 pending map 中移除（此刻自己仍是该 key 的当前在途请求）
    if (res.config.method?.toUpperCase() === 'GET') {
      pendingRequests.delete(buildRequestKey(res.config));
    }
    return res;
  },
  async (err) => {
    // 被放弃的请求（主动取消 / 已被更新的同 url 请求接管）→ **静默作废**。
    // 绝不能 reject：调用方的 .catch() 会把「旧请求被自己人取消」误报成业务失败。
    if (isCanceledError(err) || isSuperseded(err.config)) {
      return neverSettle();
    }

    const original = err.config;

    // 清除 pending 记录（仅当自己仍是该 key 的当前在途请求时才删，
    // 避免误删已被新请求接管的记录 —— 那会让新请求失去被取消的能力）
    if (original?.method?.toUpperCase() === 'GET') {
      const key = buildRequestKey(original);
      if (pendingRequests.get(key) === (original as TrackedRequestConfig).__controller) {
        pendingRequests.delete(key);
      }
    }

    // 只处理 401，且不重试 refresh 接口本身，且没有重试过
    if (err.response?.status === 401 && !original?._retry && !original?.url?.includes('/auth/refresh')) {
      const refreshToken = localStorage.getItem('refreshToken');
      if (!refreshToken) {
        logout();
        return Promise.reject(err);
      }

      if (isRefreshing) {
        // 已在刷新中，把请求加入队列，等待新 token
        return new Promise((resolve, reject) => {
          refreshQueue.push((token: string) => {
            if (!token) {
              reject(err);
            } else {
              original.headers.Authorization = `Bearer ${token}`;
              resolve(api(original));
            }
          });
        });
      }

      original._retry = true;
      isRefreshing = true;

      try {
        const res = await axios.post(`${BASE_URL}/auth/refresh`, { refresh_token: refreshToken });
        if (res.data.success) {
          const { token: newToken, refresh_token: newRefresh } = res.data;
          localStorage.setItem('token', newToken);
          localStorage.setItem('refreshToken', newRefresh);
          api.defaults.headers.common.Authorization = `Bearer ${newToken}`;
          flushQueue(newToken);
          original.headers.Authorization = `Bearer ${newToken}`;
          return api(original);
        } else {
          flushQueueReject();
          logout();
          return Promise.reject(err);
        }
      } catch {
        flushQueueReject();
        logout();
        return Promise.reject(err);
      } finally {
        isRefreshing = false;
      }
    }
    return Promise.reject(err);
  }
);

function logout() {
  localStorage.removeItem('token');
  localStorage.removeItem('refreshToken');
  localStorage.removeItem('role');
  localStorage.removeItem('user');
  window.location.href = '/login';
}

// ─── Auth ───────────────────────────────────────────
export const authApi = {
  sendCode: (email: string) =>
    api.post('/auth/send-code', { email }),
  register: (data: { username: string; email: string; password: string; code: string }) =>
    api.post('/auth/register', data),
  login: (data: { email: string; password: string }) =>
    api.post('/auth/login', data),
  refresh: (refreshToken: string) =>
    api.post('/auth/refresh', { refresh_token: refreshToken }),
  sendResetCode: (email: string) =>
    api.post('/auth/send-reset-code', { email }),
  resetPassword: (data: { email: string; code: string; new_password: string }) =>
    api.post('/auth/reset-password', data),
};

// ─── Admin ──────────────────────────────────────────
export const adminApi = {
  getStats: () => api.get('/admin/stats'),
  getTrends: () => api.get('/admin/stats/trends'),
  getMerchants: (params?: { page?: number; page_size?: number; keyword?: string; plan?: string }) =>
    api.get('/admin/merchants', { params }),
  createMerchant: (data: { username: string; email: string; password: string }) =>
    api.post('/admin/merchants', data),
  deleteMerchant: (id: string) =>
    api.delete(`/admin/merchants/${id}`),
  updateMerchantStatus: (id: string, status: string) =>
    api.patch(`/admin/merchants/${id}/status`, { status }),
  updateMerchantPlan: (id: string, plan: 'free' | 'pro', expires_days?: number) =>
    api.patch(`/admin/merchants/${id}/plan`, { plan, expires_days }),
  getPlanConfigs: () => api.get('/admin/plan-configs'),
  updatePlanConfig: (id: string, data: {
    label?: string;
    max_apps?: number;
    max_cards?: number;
    max_devices?: number;
    max_gen_once?: number;
  }) => api.patch(`/admin/plan-configs/${id}`, data),
  // 注：此前有 getApiKey / regenerateApiKey 两个方法，已随 /admin/api-key 接口一并移除
  //（该接口依赖的 admins.api_key 列从未存在于任何迁移中，是半成品）
};

// ─── Apps ───────────────────────────────────────────
export const appsApi = {
  list: (params?: { page?: number; page_size?: number }) =>
    api.get('/apps', { params }),
  create: (data: { app_name: string; description?: string }) =>
    api.post('/apps', data),
  delete: (id: string) => api.delete(`/apps/${id}`),
  updateStatus: (id: string, status: string) =>
    api.patch(`/apps/${id}/status`, { status }),
};

// ─── Cards ──────────────────────────────────────────
export const cardsApi = {
  list: (params?: { app_id?: string; status?: string; card_code?: string; page?: number; page_size?: number }) =>
    api.get('/cards', { params }),
  exportCsv: (params?: { app_id?: string; status?: string }) =>
    api.get('/cards/export', { params, responseType: 'blob' }),
  disable: (id: string) => api.patch(`/cards/${id}/disable`),
  enable: (id: string) => api.patch(`/cards/${id}/enable`),
  delete: (id: string) => api.delete(`/cards/${id}`),
  batchStatus: (ids: string[], action: 'disabled' | 'unused') =>
    api.post('/cards/batch-status', { ids, action }),
  batchExtend: (ids: string[], days: number) =>
    api.post('/cards/batch-extend', { ids, days }),
  stats: () => api.get('/cards/stats'),
  extend: (id: string, days: number) => api.patch(`/cards/${id}/extend`, { days }),
  updateNote: (id: string, note: string) => api.patch(`/cards/${id}/note`, { note }),
  generate: (data: {
    app_id: string;
    count: number;
    duration_days: number;
    max_devices: number;
    note?: string;
    prefix?: string;
    segment_count?: number;
    segment_len?: number;
  }) => api.post('/cards', data),
};

// ─── Activations ────────────────────────────────────
export const activationsApi = {
  list: (params?: { page?: number; page_size?: number; card_code?: string }) =>
    api.get('/activations', { params }),
  unbind: (id: string) => api.delete(`/activations/${id}`),
};

// ─── Merchant ───────────────────────────────────────
export const merchantApi = {
  getProfile: () => api.get('/merchant/profile'),
  dashboardStats: (range?: 'week' | 'month' | 'year') =>
    api.get('/merchant/dashboard-stats', { params: { range } }),
  changePassword: (data: { old_password: string; new_password: string }) =>
    api.post('/merchant/change-password', data),
  regenerateApiKey: () => api.post('/merchant/regenerate-apikey'),
};

// ─── Messages (Admin) ───────────────────────────────
export const adminMessagesApi = {
  list: (params?: { page?: number; page_size?: number; msg_type?: string }) =>
    api.get('/admin/messages', { params }),
  send: (data: {
    msg_type: string;
    title: string;
    content: string;
    target_type?: string;
    target_id?: string;
    target_email?: string;
    pinned?: boolean;
    expires_at?: string;
  }) => api.post('/admin/messages', data),
  update: (id: string, data: {
    title?: string;
    content?: string;
    pinned?: boolean;
    expires_at?: string;
  }) => api.patch(`/admin/messages/${id}`, data),
  delete: (id: string) => api.delete(`/admin/messages/${id}`),
};

// ─── Messages (Merchant) ────────────────────────────
export const merchantMessagesApi = {
  listNotices: (params?: { page?: number; page_size?: number }) =>
    api.get('/merchant/notices', { params }),
  listMessages: (params?: { page?: number; page_size?: number }) =>
    api.get('/merchant/messages', { params }),
  unreadCount: () => api.get('/merchant/messages/unread_count'),
  markRead: (id: string) => api.post(`/merchant/messages/${id}/read`),
};

// ─── WebSocket URL helper ────────────────────────────
export function getWsUrl(): string {
  const base = (import.meta.env.VITE_API_URL || 'http://localhost:9527') as string;
  const ws = base.replace(/^http/, 'ws');
  const token = localStorage.getItem('token') ?? '';
  return `${ws}/ws/messages?token=${encodeURIComponent(token)}`;
}

// ─── Health ──────────────────────────────────────────
export const healthApi = {
  check: () => api.get('/health'),
};

// ─── Agent ────────────────────────────────────────────
export const agentApi = {
  // 我作为上级
  createInvite: (data: { quota_total?: number; commission_rate?: number; note?: string }) =>
    api.post('/agent/invite', data),
  listAgents: (params?: { page?: number; page_size?: number }) =>
    api.get('/agent/list', { params }),
  updateQuota: (id: string, delta: number, reason?: string) =>
    api.patch(`/agent/${id}/quota`, { delta, reason }),
  updateCommission: (id: string, commission_rate: number) =>
    api.patch(`/agent/${id}/commission`, { commission_rate }),
  updateStatus: (id: string, status: 'active' | 'disabled') =>
    api.patch(`/agent/${id}/status`, { status }),
  removeAgent: (id: string) =>
    api.delete(`/agent/${id}`),
  listCommissions: (params?: { page?: number; page_size?: number }) =>
    api.get('/agent/commissions', { params }),
  // 我作为代理
  myRelation: () => api.get('/agent/my'),
  myCommissions: (params?: { page?: number; page_size?: number }) =>
    api.get('/agent/my/commissions', { params }),
  joinByInvite: (code: string) =>
    api.post(`/agent/join/${code}`),
};

// ─── Blacklist ────────────────────────────────────────
export const blacklistApi = {
  // IP 黑名单
  listIps: (params?: { page?: number; page_size?: number }) =>
    api.get('/blacklist/ips', { params }),
  addIp: (ip: string, reason?: string) =>
    api.post('/blacklist/ips', { ip, reason }),
  removeIp: (id: string) =>
    api.delete(`/blacklist/ips/${id}`),
  // 设备黑名单
  listDevices: (params?: { page?: number; page_size?: number }) =>
    api.get('/blacklist/devices', { params }),
  addDevice: (device_id: string, reason?: string) =>
    api.post('/blacklist/devices', { device_id, reason }),
  removeDevice: (id: string) =>
    api.delete(`/blacklist/devices/${id}`),
  // 异常告警
  listAlerts: (params?: { page?: number; page_size?: number }) =>
    api.get('/blacklist/alerts', { params }),
  unreadAlertCount: () =>
    api.get('/blacklist/alerts/unread_count'),
  markAlertRead: (id: string) =>
    api.post(`/blacklist/alerts/${id}/read`),
};

// ─── Webhooks ─────────────────────────────────────────
export const webhookApi = {
  get: (appId: string) => api.get(`/webhooks/app/${appId}`),
  upsert: (appId: string, data: { url: string; secret?: string; enabled?: boolean; events?: string[] }) =>
    api.put(`/webhooks/app/${appId}`, data),
  delete: (appId: string) => api.delete(`/webhooks/app/${appId}`),
  list: () => api.get('/webhooks'),
};

// ─── Payments ─────────────────────────────────────────
export const paymentsApi = {
  create: (data: { pay_type: string; plan_id?: string; expires_days?: number; channel?: string }) =>
    api.post('/pay/auth/create', data),
  list: (params?: { page?: number; page_size?: number }) =>
    api.get('/pay/auth/orders', { params }),
  getStatus: (orderId: string) =>
    api.get('/pay/auth/status', { params: { order_id: orderId } }),
  cancel: (data: { order_id: string }) =>
    api.post('/pay/auth/cancel', data),
};

// ─── Subscription Plans ────────────────────────────────
export const plansApi = {
  list: (params?: { enabled_only?: boolean }) =>
    api.get('/admin/subscription-plans', { params }),
  listEnabled: () =>
    api.get('/pay/auth/plans'),
  create: (data: {
    plan: string;
    name: string;
    days?: number | null;
    price: number;
    original_price?: number | null;
    badge?: string | null;
    highlight?: boolean;
    sort_order?: number;
    enabled?: boolean;
  }) => api.post('/admin/subscription-plans', data),
  update: (id: string, data: {
    name?: string;
    days?: number | null;
    price?: number;
    original_price?: number | null;
    badge?: string | null;
    highlight?: boolean;
    sort_order?: number;
    enabled?: boolean;
  }) => api.put(`/admin/subscription-plans/${id}`, data),
  remove: (id: string) => api.delete(`/admin/subscription-plans/${id}`),
};

// ─── APK Shield ─────────────────────────────────────────
export const apkShieldApi = {
  upload: (file: File, appId: string, merchantId: string, apiKey: string, appName?: string) => {
    const form = new FormData();
    form.append('apk', file);
    return shieldApi.post('/upload', form, {
      params: {
        app_id: appId,
        merchant_id: merchantId,
        api_key: apiKey,
        ...(appName && { app_name: appName }),
      },
      headers: { 'Content-Type': 'multipart/form-data' },
    });
  },
  health: () =>
    shieldApi.get('/health'),
  list: (merchantId: string, page = 1, pageSize = 20) =>
    shieldApi.get('/jobs', { params: { merchant_id: merchantId, page, page_size: pageSize } }),
  status: (jobId: string) =>
    shieldApi.get(`/status/${jobId}`),
  download: (jobId: string) =>
    shieldApi.get(`/download/${jobId}`, { responseType: 'blob' }),
};

// ─── EXE Shield ─────────────────────────────────────────
export const exeShieldApi = {
  upload: (file: File, appId: string, merchantId: string, apiKey: string, appName?: string, windowTitle?: string, windowHint?: string) => {
    return shieldApi.post('/exe/upload', file, {
      params: {
        app_id: appId,
        merchant_id: merchantId,
        api_key: apiKey,
        ...(appName && { app_name: appName }),
        ...(windowTitle && { window_title: windowTitle }),
        ...(windowHint && { window_hint: windowHint }),
      },
      headers: { 'Content-Type': 'application/octet-stream' },
    });
  },
  health: () =>
    shieldApi.get('/exe/health'),
  list: (merchantId: string, page = 1, pageSize = 20) =>
    shieldApi.get('/exe/jobs', { params: { merchant_id: merchantId, page, page_size: pageSize } }),
  status: (jobId: string) =>
    shieldApi.get(`/exe/status/${jobId}`),
  download: (jobId: string) =>
    shieldApi.get(`/exe/download/${jobId}`, { responseType: 'blob' }),
};

// ─── OAuth ───────────────────────────────────────────────
export const oauthApi = {
  // 获取已启用的 OAuth 提供商列表
  listProviders: () => api.get('/oauth/providers'),
  // 发起 OAuth 授权
  authorize: (provider: string) => api.get(`/oauth/${provider}/authorize`),
};
