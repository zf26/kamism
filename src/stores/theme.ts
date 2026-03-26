import { create } from 'zustand';
import { persist } from 'zustand/middleware';

type Theme = 'dark' | 'light';

interface ThemeStore {
  theme: Theme;
  toggle: () => void;
  setTheme: (t: Theme) => void;
}

export const useThemeStore = create<ThemeStore>()(
  persist(
    (set, get) => ({
      theme: 'dark',
      toggle: () => {
        const next = get().theme === 'dark' ? 'light' : 'dark';
        set({ theme: next });
        document.documentElement.setAttribute('data-theme', next);
      },
      setTheme: (t) => {
        set({ theme: t });
        document.documentElement.setAttribute('data-theme', t);
      },
    }),
    { name: 'kamism-theme' }
  )
);

/** 应用启动时同步主题到 <html data-theme> */
export function applyStoredTheme() {
  const raw = localStorage.getItem('kamism-theme');
  const theme: Theme = raw ? (JSON.parse(raw)?.state?.theme ?? 'dark') : 'dark';
  document.documentElement.setAttribute('data-theme', theme);
}

