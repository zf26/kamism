import { create } from 'zustand';
import type { WsEvent } from '../hooks/useWs';

/**
 * WS 事件总线 store
 * Layout 收到 WS 消息后写入 lastEvent，
 * 其他组件订阅此 store 获取事件，无需自己建 WS 连接。
 */
interface WsEventState {
  lastEvent: WsEvent | null;
  setLastEvent: (evt: WsEvent) => void;
}

export const useWsEventStore = create<WsEventState>((set) => ({
  lastEvent: null,
  setLastEvent: (evt) => set({ lastEvent: evt }),
}));

