import { create } from 'zustand';

export interface User {
  id: string;
  username: string;
  email: string;
  api_key?: string;
  status?: string;
  email_verified?: boolean;
  created_at?: string;
}

interface AuthState {
  token: string | null;
  refreshToken: string | null;
  role: 'admin' | 'merchant' | null;
  user: User | null;
  setAuth: (token: string, refreshToken: string, role: 'admin' | 'merchant', user: User) => void;
  updateToken: (token: string, refreshToken: string) => void;
  logout: () => void;
  isAuthenticated: () => boolean;
}

export const useAuthStore = create<AuthState>((set, get) => ({
  token: localStorage.getItem('token'),
  refreshToken: localStorage.getItem('refreshToken'),
  role: localStorage.getItem('role') as 'admin' | 'merchant' | null,
  user: (() => {
    try {
      const u = localStorage.getItem('user');
      return u ? JSON.parse(u) : null;
    } catch {
      return null;
    }
  })(),

  setAuth: (token, refreshToken, role, user) => {
    localStorage.setItem('token', token);
    localStorage.setItem('refreshToken', refreshToken);
    localStorage.setItem('role', role);
    localStorage.setItem('user', JSON.stringify(user));
    set({ token, refreshToken, role, user });
  },

  updateToken: (token, refreshToken) => {
    localStorage.setItem('token', token);
    localStorage.setItem('refreshToken', refreshToken);
    set({ token, refreshToken });
  },

  logout: () => {
    localStorage.removeItem('token');
    localStorage.removeItem('refreshToken');
    localStorage.removeItem('role');
    localStorage.removeItem('user');
    set({ token: null, refreshToken: null, role: null, user: null });
  },

  isAuthenticated: () => !!get().token,
}));
