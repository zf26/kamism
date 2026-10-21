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

// ── Token 续期：全站**单飞**（single-flight）──────────────────────────────
//
// 为什么必须单飞，而不是各调用点自己刷（这里踩过坑，记下来）：
//   触发续期的地方有两个 —— ① REST 请求拿到 401；② WebSocket 断线重连前主动续期
//   （见 hooks/useWs.ts）。两者可能同时发生，各自发一次 /auth/refresh，于是服务端
//   收到多个并发的 refresh，各自滚动签发新的 refresh token；localStorage 最后只留
//   下「最后完成的那次」的结果，其余响应里的 refresh token 直接作废浪费。
//   本项目服务端**不吊销旧 refresh**（只验签 + 校验 token_version，见
//   auth.rs::refresh_token），所以并发不会互相踢下线，但每多一次就是一次多余的
//   DB + Redis 查询。单飞之后：一次续期只发一个请求，所有等待方共享同一个结果。
let refreshInFlight: Promise<RefreshOutcome> | null = null;

/**
 * 续期结果。
 *
 * ⚠️ 必须区分「凭据失效」和「服务端临时故障」：调用方拿它决定**是登出还是重试**。
 * 混为一谈的后果很严重 —— 服务端 Redis 抖一下（refresh 走 fail-closed 会返回
 * 「服务暂时不可用」），如果当成「凭据失效」就会把全站用户踢到登录页。
 */
export type RefreshOutcome = {
  /** 新 access token；失败为 null */
  token: string | null;
  /** true = 服务端明确拒绝（令牌无效/过期/被吊销/账号禁用），只能重新登录 */
  authLost: boolean;
};

/** 从 JWT 里读 exp（秒）。**不验签** —— 只用于判断「要不要提前续期」，不能当安全依据。 */
function readJwtExp(token: string): number | null {
  try {
    const part = token.split('.')[1];
    if (!part) return null;
    const b64 = part.replace(/-/g, '+').replace(/_/g, '/');
    const padded = b64.padEnd(Math.ceil(b64.length / 4) * 4, '=');
    const bin = atob(padded);
    const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
    const payload = JSON.parse(new TextDecoder().decode(bytes)) as { exp?: unknown };
    return typeof payload.exp === 'number' ? payload.exp : null;
  } catch {
    return null;
  }
}

/** token 剩余有效期（毫秒）；非 JWT / 解析失败返回 null（不猜，交给服务端裁决）。 */
export function getTokenRemainingMs(token: string | null): number | null {
  if (!token) return null;
  const exp = readJwtExp(token);
  if (exp === null) return null;
  return exp * 1000 - Date.now();
}

/**
 * token 是否已过期或即将过期（默认提前 120 秒算「要过期了」）。
 *
 * 余量的意义：网络往返 + 服务端处理要时间，卡在「还剩 3 秒」时建连，
 * 到达服务端时可能刚好过期 —— 这种 401 最难查（本地看 token 明明没过期）。
 */
export function isTokenExpiring(token: string | null, skewMs = 120_000): boolean {
  if (!token) return true;
  const remaining = getTokenRemainingMs(token);
  if (remaining === null) return false;
  return remaining <= skewMs;
}

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
      original._retry = true;
      // 单飞续期：并发多个 401、以及 WebSocket 重连前的主动续期，共用同一次请求
      const outcome = await refreshSessionOutcome();

      if (outcome.token) {
        original.headers.Authorization = `Bearer ${outcome.token}`;
        return api(original);
      }

      // 只有**确认**凭据失效才登出。服务端临时故障（Redis/MQ 抖动、5xx）时
      // 不能把人踢下线 —— 那会把一次基础设施抖动放大成「全站被强制登出」。
      if (outcome.authLost) logout();
      return Promise.reject(err);
    }
    return Promise.reject(err);
  }
);

/** 清空本地会话并跳登录页。WS 侧确认会话失效时也会调它（见 hooks/useWs.ts）。 */
export function logout() {
  localStorage.removeItem('token');
  localStorage.removeItem('refreshToken');
  localStorage.removeItem('role');
  localStorage.removeItem('user');
  window.location.href = '/login';
}

/** 写入新 token 并同步 axios 默认头（续期成功后唯一的落盘点）。 */
function applyToken(token: string, nextRefreshToken?: string) {
  localStorage.setItem('token', token);
  if (nextRefreshToken) localStorage.setItem('refreshToken', nextRefreshToken);
  api.defaults.headers.common.Authorization = `Bearer ${token}`;
}

/**
 * 续期，并告诉调用方失败的类型（凭据失效 vs 临时故障）。
 *
 * 单飞：并发调用共享同一个 Promise，只有第一个真正发请求。
 *
 * 用裸 `axios` 而不是 `api` 实例：后者会再过一遍请求/响应拦截器，
 * 一旦续期本身返回 401 就会递归调用本函数。
 */
export function refreshSessionOutcome(): Promise<RefreshOutcome> {
  if (refreshInFlight) return refreshInFlight;

  const storedRefresh = localStorage.getItem('refreshToken');
  if (!storedRefresh) {
    // 没有 refresh token = 本地会话不完整，重试也不会有结果
    return Promise.resolve({ token: null, authLost: true });
  }

  refreshInFlight = (async (): Promise<RefreshOutcome> => {
    try {
      const res = await axios.post(`${BASE_URL}/auth/refresh`, { refresh_token: storedRefresh });
      const body = (res.data ?? {}) as { success?: boolean; token?: string; refresh_token?: string; message?: string };

      if (body.success && typeof body.token === 'string' && body.token) {
        applyToken(body.token, typeof body.refresh_token === 'string' ? body.refresh_token : undefined);
        return { token: body.token, authLost: false };
      }

      // refresh 接口在业务失败时也回 200，只能看 message。这里**宁可判成临时故障**：
      // 误判成「临时故障」最坏是多重试几次（有上限），误判成「凭据失效」会把人踢下线。
      const message = typeof body.message === 'string' ? body.message : '';
      const authLost = /无效|过期|失效|禁用|重新登录/.test(message) && !/稍后重试|暂时不可用/.test(message);
      return { token: null, authLost };
    } catch (e) {
      // HTTP 层的 4xx 才是凭据问题；5xx / 超时 / 断网都属于临时故障
      const status = (e as { response?: { status?: number } })?.response?.status;
      const authLost = status === 400 || status === 401 || status === 403;
      return { token: null, authLost };
    } finally {
      refreshInFlight = null;
    }
  })();

  return refreshInFlight;
}

/** 续期并只要 token（大多数调用方只关心这个）。 */
export async function refreshSession(): Promise<string | null> {
  const outcome = await refreshSessionOutcome();
  return outcome.token;
}

// ─── Auth ───────────────────────────────────────────
export const authApi = {
  sendCode: (email: string) =>
    api.post('/auth/send-code', { email }),
  // 人机验证题面：答案只存在服务端，这里拿到的只有 id 和题面
  getCaptcha: () =>
    api.get('/auth/captcha'),
  register: (data: {
    username: string; email: string; password: string; code: string;
    captcha_id: string; captcha_answer: string;
  }) =>
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
  // 续期：在已有到期时间上叠加天数（与 updateMerchantPlan 现在是同一套语义，
  // 都走后端 utils::plan::extend_plan；保留两个入口是因为界面上的场景不同）
  renewMerchantPlan: (id: string, days: number) =>
    api.patch(`/admin/merchants/${id}/renew`, { days }),
  // 管理员改自己的密码（成功后返回新的 token 对，需替换本地凭据）
  changeAdminPassword: (data: { old_password: string; new_password: string }) =>
    api.patch('/admin/password', data),
  // 审计日志（只读，写入由后端中间件自动完成）
  getAuditLogs: (params?: { page?: number; page_size?: number; admin_id?: string; keyword?: string }) =>
    api.get('/admin/audit-logs', { params }),
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
//
// 浏览器无法给 WebSocket 加 Authorization 头，只能把 token 放 query。
// `token` 参数不是为了方便 —— 是为了**消除竞态**：调用方先用
// `refreshSessionOutcome` 拿到一个确定有效的 token，再把它传进来；
// 如果这里自己去 localStorage 读，读到的可能是别的标签页刚刷新前的旧值。
export function getWsUrl(token: string = localStorage.getItem('token') ?? ''): string {
  const base = (import.meta.env.VITE_API_URL || 'http://localhost:9527') as string;
  const ws = base.replace(/^http/, 'ws');
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
