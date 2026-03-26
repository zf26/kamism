import { useEffect, useRef, useCallback } from 'react';
import { getWsUrl } from '../lib/api';

export type WsEvent = {
  event: string;
  data?: Record<string, unknown>;
};

type WsOptions = {
  onMessage?: (evt: WsEvent) => void;
  onOpen?: () => void;
  onClose?: () => void;
  /** 断线重连间隔（ms），默认 3000，设为 0 禁用 */
  reconnectInterval?: number;
};

/**
 * useWs — 商户端 WebSocket 连接 hook
 *
 * 回调函数通过 ref 存储，避免内联函数引用变化导致连接反复重建。
 * connect 函数稳定，仅在组件挂载时建立一次连接。
 */
export function useWs(options: WsOptions = {}) {
  const { reconnectInterval = 3000 } = options;

  // reconnectInterval < 0 表示完全禁用（不建立初始连接）
  const disabled = reconnectInterval < 0;

  // 用 ref 存回调，避免因闭包引用变化触发重连
  const onMessageRef = useRef(options.onMessage);
  const onOpenRef    = useRef(options.onOpen);
  const onCloseRef   = useRef(options.onClose);
  useEffect(() => { onMessageRef.current = options.onMessage; });
  useEffect(() => { onOpenRef.current    = options.onOpen; });
  useEffect(() => { onCloseRef.current   = options.onClose; });

  const wsRef        = useRef<WebSocket | null>(null);
  const retryRef     = useRef<number>(0);
  const timerRef     = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unmountedRef = useRef(false);
  const intervalRef  = useRef(reconnectInterval);
  useEffect(() => { intervalRef.current = reconnectInterval; });

  // connect 无外部依赖，永远稳定，不会触发 useEffect 重跑
  const connect = useCallback(() => {
    if (unmountedRef.current) return;

    const url = getWsUrl();
    const ws = new WebSocket(url);
    wsRef.current = ws;

    ws.onopen = () => {
      retryRef.current = 0;
      onOpenRef.current?.();
    };

    ws.onmessage = (e) => {
      try {
        const parsed: WsEvent = JSON.parse(e.data as string);
        onMessageRef.current?.(parsed);
      } catch {
        // 非 JSON 帧忽略
      }
    };

    ws.onclose = () => {
      onCloseRef.current?.();
      if (unmountedRef.current || intervalRef.current === 0) return;
      // 指数退避：3s → 6s → 12s → 最大 30s
      const delay = Math.min(intervalRef.current * 2 ** retryRef.current, 30_000);
      retryRef.current += 1;
      timerRef.current = setTimeout(connect, delay);
    };

    ws.onerror = () => {
      ws.close();
    };
  }, []); // 空依赖：connect 永远稳定

  useEffect(() => {
    if (disabled) return;
    unmountedRef.current = false;
    connect();
    return () => {
      unmountedRef.current = true;
      if (timerRef.current) clearTimeout(timerRef.current);
      wsRef.current?.close();
    };
  }, [connect, disabled]); // connect 稳定，effect 只跑一次

  const send = useCallback((data: unknown) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(data));
    }
  }, []);

  return { send };
}
