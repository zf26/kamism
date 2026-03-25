import axios from 'axios';

// API 地址从环境变量读取
// 开发环境：npm run tauri dev 时从 .env.development 读取
// 生产环境：npm run tauri build 时从 .env.production 读取
// 回退值：http://localhost:9527
const BASE_URL = import.meta.env.VITE_API_URL || 'http://localhost:9527';
export const api = axios.create({
  baseURL: BASE_URL,
  timeout: 15000,
});

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

// 请求拦截器：自动携带 token
api.interceptors.request.use((config) => {
  const token = localStorage.getItem('token');
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

// 响应拦截器：401 时自动用 refresh_token 续期，无感刷新
api.interceptors.response.use(
  (res) => res,
  async (err) => {
    const original = err.config;
    // 只处理 401，且不重试 refresh 接口本身，且没有重试过
    if (err.response?.status === 401 && !original._retry && !original.url?.includes('/auth/refresh')) {
      const refreshToken = localStorage.getItem('refreshToken');
      if (!refreshToken) {
        // 没有 refresh token，直接跳登录
        logout();
        return Promise.reject(err);
      }

      if (isRefreshing) {
        // 已在刷新中，把请求加入队列，等待新 token
        return new Promise((resolve) => {
          refreshQueue.push((token: string) => {
            original.headers.Authorization = `Bearer ${token}`;
            resolve(api(original));
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
          logout();
          return Promise.reject(err);
        }
      } catch {
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
  getMerchants: (params?: { page?: number; page_size?: number; keyword?: string; plan?: string }) =>
    api.get('/admin/merchants', { params }),
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
  list: (params?: { app_id?: string; status?: string; page?: number; page_size?: number }) =>
    api.get('/cards', { params }),
  generate: (data: {
    app_id: string;
    count: number;
    duration_days: number;
    max_devices: number;
    note?: string;
  }) => api.post('/cards', data),
  disable: (id: string) => api.patch(`/cards/${id}/disable`),
  enable: (id: string) => api.patch(`/cards/${id}/enable`),
  delete: (id: string) => api.delete(`/cards/${id}`),
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
  changePassword: (data: { old_password: string; new_password: string }) =>
    api.post('/merchant/change-password', data),
  regenerateApiKey: () => api.post('/merchant/regenerate-apikey'),
};

// ─── Messages (Admin) ───────────────────────────────
export const adminMessagesApi = {
  list: (params?: { page?: number; page_size?: number; msg_type?: string }) =>
    api.get('/api/admin/messages', { params }),
  send: (data: {
    msg_type: string;
    title: string;
    content: string;
    target_type?: string;
    target_id?: string;
    target_email?: string;
    pinned?: boolean;
    expires_at?: string;
  }) => api.post('/api/admin/messages', data),
  update: (id: string, data: {
    title?: string;
    content?: string;
    pinned?: boolean;
    expires_at?: string;
  }) => api.patch(`/api/admin/messages/${id}`, data),
  delete: (id: string) => api.delete(`/api/admin/messages/${id}`),
};

// ─── Messages (Merchant) ────────────────────────────
export const merchantMessagesApi = {
  listNotices: (params?: { page?: number; page_size?: number }) =>
    api.get('/api/merchant/notices', { params }),
  listMessages: (params?: { page?: number; page_size?: number }) =>
    api.get('/api/merchant/messages', { params }),
  unreadCount: () => api.get('/api/merchant/messages/unread_count'),
  markRead: (id: string) => api.post(`/api/merchant/messages/${id}/read`),
};

// ─── WebSocket URL helper ────────────────────────────
export function getWsUrl(): string {
  const base = (import.meta.env.VITE_API_URL || 'http://localhost:9527') as string;
  const ws = base.replace(/^http/, 'ws');
  const token = localStorage.getItem('token') ?? '';
  return `${ws}/api/ws/messages?token=${encodeURIComponent(token)}`;
}
