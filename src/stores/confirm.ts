import { create } from 'zustand';

interface ConfirmOptions {
  title?: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
}

interface ConfirmState {
  open: boolean;
  options: ConfirmOptions;
  resolve: ((ok: boolean) => void) | null;
  // 弹出确认框，返回 Promise<boolean>
  confirm: (options: ConfirmOptions) => Promise<boolean>;
  _accept: () => void;
  _cancel: () => void;
}

export const useConfirmStore = create<ConfirmState>((set, get) => ({
  open: false,
  options: { message: '' },
  resolve: null,

  confirm: (options) =>
    new Promise<boolean>((res) => {
      set({ open: true, options, resolve: res });
    }),

  _accept: () => {
    get().resolve?.(true);
    set({ open: false, resolve: null });
  },

  _cancel: () => {
    get().resolve?.(false);
    set({ open: false, resolve: null });
  },
}));

/** 便捷 hook，只暴露调用方需要的 confirm 函数 */
export function useConfirm() {
  return useConfirmStore((s) => s.confirm);
}

