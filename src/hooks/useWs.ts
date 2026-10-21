import { useEffect, useRef, useCallback } from 'react';
import {
  getWsUrl,
  getTokenRemainingMs,
  isTokenExpiring,
  refreshSessionOutcome,
} from '../lib/api';

export type WsEvent = {
  event: string;
  data?: Record<string, unknown>;
};

type WsOptions = {
  onMessage?: (evt: WsEvent) => void;
  onOpen?: () => void;
  onClose?: () => void;
  /**
   * 会话被服务端**确认**失效（refresh token 也换不到新令牌）时触发。
   * 一般传 `logout` —— 不处理的话页面会停在「看起来已登录、其实收不到推送」的状态。
   * 注意：只有确认失效才触发；服务端临时故障（5xx / 断网）不会触发，避免误踢。
   */
  onAuthLost?: () => void;
  /** 断线重连起始间隔（ms），默认 3000；0 = 只连一次不重连；负数 = 完全不建立连接 */
  reconnectInterval?: number;
};

/** 连续失败多少次后降频到「5 分钟一次」的慢探测 */
const MAX_ATTEMPTS = 8;
/** 退避上限 */
const MAX_BACKOFF_MS = 30_000;
/** 慢探测间隔（放弃自动重连后仍保留的自愈通道） */
const SLOW_PROBE_MS = 5 * 60_000;
/** 建连前要求的 token 最小剩余有效期 */
const TOKEN_SKEW_MS = 2 * 60_000;
/** 连接活过这么久才算「健康断开」；短于此视为建连失败，走失败计数 */
const HEALTHY_CONNECTION_MS = 1000;

/**
 * useWs — 商户端 WebSocket 连接 hook
 *
 * ── 为什么重连逻辑写这么啰嗦 ───────────────────────────────────────────────
 *
 * 旧实现只有一句「close 了就 3 秒后重连」。上线后暴露出一个很贵的问题：
 *
 *   access token 只有 2 小时。前端只在**收到 REST 401** 时才续期，
 *   而 WebSocket 只要连着就一个请求都不发 —— 于是标签页开着、用户没操作、
 *   token 在后台静静过期。此时任何一次断线（休眠唤醒 / Wi-Fi 漫游 / 代理超时 /
 *   服务端重启）都会触发重连，而重连带的是**已过期的 token** → 服务端 401
 *   → 浏览器 close → 3 秒后再来一次。
 *
 *   重连没有次数上限，于是每个「开着页面去吃饭」的用户，其浏览器都会
 *   以每 30 秒一次的节奏持续打 401，直到用户关掉页面。服务端白白消耗
 *   TCP 握手、JWT 验签和反代日志，而客户端永远连不上（功能也是坏的）。
 *
 * 现在的三条约束把这个循环彻底掐掉：
 *
 *   ① **握手前先保证 token 有效**（`resolveToken`）。过期就续期，
 *      续不出来就不发握手 —— 过期 token 根本到不了服务端，401 从源头消失。
 *   ② **连接存活期间提前续期**（`scheduleTokenRefresh`）。token 到期前
 *      5 分钟就把 localStorage 里的那份换掉，这样「断线时刚好过期」不存在。
 *   ③ **失败有上限**。连续失败 8 次后降到 5 分钟一次慢探测；后台标签页
 *      和离线状态**完全不重连**，等 visibilitychange / online 唤醒。
 *
 * 回调函数通过 ref 存储，避免内联函数引用变化导致连接反复重建。
 */
export function useWs(options: WsOptions = {}) {
  const { reconnectInterval = 3000 } = options;

  // reconnectInterval < 0 表示完全禁用（不建立初始连接）
  const disabled = reconnectInterval < 0;

  // 用 ref 存回调，避免因闭包引用变化触发重连
  const onMessageRef = useRef(options.onMessage);
  const onOpenRef = useRef(options.onOpen);
  const onCloseRef = useRef(options.onClose);
  const onAuthLostRef = useRef(options.onAuthLost);
  useEffect(() => { onMessageRef.current = options.onMessage; });
  useEffect(() => { onOpenRef.current = options.onOpen; });
  useEffect(() => { onCloseRef.current = options.onClose; });
  useEffect(() => { onAuthLostRef.current = options.onAuthLost; });

  const socketRef = useRef<WebSocket | null>(null);
  const intervalRef = useRef(reconnectInterval);
  useEffect(() => { intervalRef.current = reconnectInterval; });

  useEffect(() => {
    if (disabled) return;

    // ── 本 effect 私有的可变状态 ────────────────────────────────────────
    // 全部放在闭包里而不是 ref 里：连接状态机只属于「本次挂载」，
    // 卸载即整体丢弃，不会有跨挂载的脏状态（React StrictMode 会把 effect
    // 跑两遍，这一点很关键）。
    let alive = true;       // 卸载后置 false，拦住所有在途的异步回调
    let stopped = false;    // 会话确认失效，不再重连
    let attempt = 0;        // 连续失败次数（退避指数 / 降频阈值）
    let refreshed = false;  // 本轮故障中是否已强制续期过一次
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    let tokenTimer: ReturnType<typeof setTimeout> | null = null;

    const clearRetry = () => {
      if (retryTimer) { clearTimeout(retryTimer); retryTimer = null; }
    };
    const clearTokenTimer = () => {
      if (tokenTimer) { clearTimeout(tokenTimer); tokenTimer = null; }
    };

    function backoffDelay() {
      const base = Math.max(intervalRef.current, 1000);
      return Math.min(base * 2 ** Math.min(attempt, 5), MAX_BACKOFF_MS);
    }

    function scheduleRetry(delay: number) {
      if (!alive || stopped || intervalRef.current === 0) return;
      clearRetry();
      // 抖动 ±20%：服务端重启时所有客户端会在同一瞬间断线，
      // 没有抖动的话它们会在同一瞬间重连，打出一个没必要的尖峰。
      const jittered = Math.round(delay * (0.8 + Math.random() * 0.4));
      retryTimer = setTimeout(() => {
        retryTimer = null;
        void connect();
      }, jittered);
    }

    function stopForAuthLost() {
      if (stopped) return;
      stopped = true;
      clearRetry();
      clearTokenTimer();
      const ws = socketRef.current;
      socketRef.current = null;
      try { ws?.close(); } catch { /* 已经关闭 */ }
      onAuthLostRef.current?.();
    }

    /**
     * 取一个「确定还能用」的 token。三条出路：
     *   ok        —— 可以建连
     *   auth-lost —— 凭据确实失效，应停止重连并登出
     *   transient —— 服务端临时故障 / 断网，应该退避重试（**不能登出**）
     */
    type TokenOutcome =
      | { kind: 'ok'; token: string }
      | { kind: 'auth-lost' }
      | { kind: 'transient' };

    async function resolveToken(): Promise<TokenOutcome> {
      const cached = localStorage.getItem('token');
      if (cached && !isTokenExpiring(cached, TOKEN_SKEW_MS)) {
        return { kind: 'ok', token: cached };
      }

      const outcome = await refreshSessionOutcome();
      if (outcome.token) return { kind: 'ok', token: outcome.token };
      if (outcome.authLost) return { kind: 'auth-lost' };

      // 续期因临时故障失败，但手里的 token 其实还没真过期 —— 照样用它，
      // 服务端此刻仍然认。只有在「真的已经过期」时才必须等续期成功。
      if (cached && !isTokenExpiring(cached, 0)) return { kind: 'ok', token: cached };
      return { kind: 'transient' };
    }

    /**
     * 连接存活期间的「预防性续期」。
     *
     * 这是整套改动里最关键的一环：WS 只在**握手时**校验 token，连接不断就
     * 永远不会 401；真正致命的是「token 过期了而连接刚好断掉」。
     * 所以趁连接还在，把 localStorage 里的 token 换成新的 ——
     * 之后无论什么时候断线，重连带的都是一个有 2 小时余量的 token。
     */
    function scheduleTokenRefresh(token: string | null, fixedDelay?: number) {
      clearTokenTimer();

      let delay = fixedDelay;
      if (delay === undefined) {
        const remaining = getTokenRemainingMs(token);
        // 不是标准 JWT / 解不出来：不猜，反正握手时服务端会裁决
        if (remaining === null) return;
        // 常规：还剩 2 小时 → 提前 5 分钟续；本身已经不足 10 分钟 → 取剩余时间的一半
        const lead = Math.min(5 * 60_000, Math.max(remaining * 0.5, 10_000));
        delay = Math.max(remaining - lead, 5_000);
      }

      tokenTimer = setTimeout(() => {
        tokenTimer = null;
        void (async () => {
          if (!alive || stopped) return;
          const outcome = await refreshSessionOutcome();
          if (!alive || stopped) return;

          if (outcome.token) { scheduleTokenRefresh(outcome.token); return; }
          if (outcome.authLost) { stopForAuthLost(); return; }
          // 临时故障：不因此断开（服务端仍认现在这个 token），2 分钟后再试
          scheduleTokenRefresh(null, 120_000);
        })();
      }, delay);
    }

    /**
     * 连接断开后的决策。
     * @param livedMs 这次连接活了多久。0 = 握手就没成功；
     *                很短（<1s）= 服务端刚接受就掐断，同样按失败算 ——
     *                否则「open 立刻 close」会被当成健康断线，退避永远回到 3 秒，
     *                变成每 3 秒一次的握手热循环。
     */
    async function handleDrop(livedMs: number) {
      if (!alive || stopped) return;
      if (intervalRef.current === 0) return; // 调用方要求「只连一次」

      if (livedMs >= HEALTHY_CONNECTION_MS) {
        // 正常断开：退避从头开始，3 秒后就重连
        attempt = 0;
        refreshed = false;
      } else {
        attempt = Math.min(attempt + 1, MAX_ATTEMPTS);

        // 握手失败、但本地 token 看起来还有效 → 可能是**已被吊销**
        // （改密码 / 封号 / 重置密钥都会让 token_version 前进）。
        // 强制续期一次：续期接口会重新走一遍吊销校验，能明确告诉我们
        // 「是凭据死了」还是「服务端只是抽了一下」。
        if (!refreshed && attempt >= 2) {
          refreshed = true;
          const outcome = await refreshSessionOutcome();
          if (!alive || stopped) return;
          if (outcome.token) {
            attempt = 0;
            scheduleRetry(500); // 拿到新 token，立刻重试（不用等退避）
            return;
          }
          if (outcome.authLost) { stopForAuthLost(); return; }
          // transient：继续走退避，不要登出
        }

        if (attempt >= MAX_ATTEMPTS) {
          // 不彻底放弃：降频到 5 分钟一次。
          // 「网络恢复了但没触发 online 事件」（部分企业网/驱动）这种情况，
          // 只有慢探测能救回来；同时把无谓的请求从每 30 秒压到每 5 分钟。
          scheduleRetry(SLOW_PROBE_MS);
          return;
        }
      }

      scheduleRetry(backoffDelay());
    }

    async function connect() {
      if (!alive || stopped) return;

      // ── 后台 / 离线时不发握手 ──────────────────────────────────────
      // 后台标签页的定时器会被浏览器节流到「每分钟最多一次」，断线后在这里
      // 反复重试毫无意义，只会持续打服务端。切回前台时 visibilitychange 会补连。
      if (!navigator.onLine) return;
      if (document.hidden) return;

      const existing = socketRef.current;
      if (existing && (existing.readyState === WebSocket.OPEN || existing.readyState === WebSocket.CONNECTING)) {
        return; // 已经连着或正在连，别开第二条
      }

      const resolved = await resolveToken();
      if (!alive || stopped) return;
      if (resolved.kind === 'auth-lost') { stopForAuthLost(); return; }
      if (resolved.kind === 'transient') {
        // 连 token 都拿不到，就别去握手了 —— 这一条正是「401 风暴」的闸门
        attempt = Math.min(attempt + 1, MAX_ATTEMPTS);
        scheduleRetry(attempt >= MAX_ATTEMPTS ? SLOW_PROBE_MS : backoffDelay());
        return;
      }

      const ws = new WebSocket(getWsUrl(resolved.token));
      socketRef.current = ws;
      let openedAt = 0;

      ws.onopen = () => {
        openedAt = Date.now();
        attempt = 0;
        refreshed = false;
        scheduleTokenRefresh(resolved.token);
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

      // 出错后浏览器必定紧跟着发 close，统一在 onclose 里处理；
      // 这里只兜底「万一没跟」的情况，防止连接卡在 CLOSING 状态没人收尾。
      ws.onerror = () => {
        if (ws.readyState === WebSocket.CLOSING || ws.readyState === WebSocket.CLOSED) return;
        try { ws.close(); } catch { /* 忽略 */ }
      };

      ws.onclose = () => {
        const stillCurrent = socketRef.current === ws;
        // 已经被新连接取代（例如 resume() 抢先建了新连接）时，这条只是过期事件的
        // 收尾：既不能清掉新连接的续期定时器，也不能去动它的退避状态。
        if (!stillCurrent) return;
        socketRef.current = null;
        clearTokenTimer();
        if (!alive) return;
        onCloseRef.current?.();
        void handleDrop(openedAt ? Date.now() - openedAt : 0);
      };
    }

    /** 页面重新可见 / 网络恢复：立刻恢复连接，并重置退避 */
    function resume() {
      if (!alive || stopped) return;
      if (document.hidden || !navigator.onLine) return;
      const ws = socketRef.current;
      if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
      clearRetry();
      attempt = 0;
      refreshed = false;
      void connect();
    }

    const onVisibilityChange = () => { if (!document.hidden) resume(); };
    const onOnline = () => resume();

    document.addEventListener('visibilitychange', onVisibilityChange);
    window.addEventListener('online', onOnline);

    void connect();

    return () => {
      alive = false;
      clearRetry();
      clearTokenTimer();
      document.removeEventListener('visibilitychange', onVisibilityChange);
      window.removeEventListener('online', onOnline);
      const ws = socketRef.current;
      socketRef.current = null;
      try { ws?.close(); } catch { /* 已经关闭 */ }
    };
  }, [disabled]);

  const send = useCallback((data: unknown) => {
    const ws = socketRef.current;
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(data));
    }
  }, []);

  return { send };
}
