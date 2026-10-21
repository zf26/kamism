import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async ({ mode }) => {
  // 读 .env.<mode>（含非 VITE_ 前缀的变量），用于取 VITE_BASE
  // @ts-expect-error process is a nodejs global
  const env = loadEnv(mode, process.cwd(), '');
  // @ts-expect-error process is a nodejs global
  const base = process.env.VITE_BASE || env.VITE_BASE || '/';
  // 构建日志里打一行，避免「以为配了、其实没生效」
  console.log(`[vite] base = ${base}`);

  return {
    plugins: [
      react(),
    ],
    // 资源 URL 前缀，必须和「前端部署到 URL 的哪一层」一致：
    //   站点根目录        → '/'（默认）
    //   子路径 /kamism/   → 构建时前置 VITE_BASE=/kamism/
    // 不要写进 .env.production：`npm run build` 同时被 Tauri 打包复用，桌面端会跟着变成 /kamism/。
    // 路由 basename、登出跳转、地图数据 URL 都由 import.meta.env.BASE_URL 推导，改这一处即可。
    base,

    // ── 构建优化 ──────────────────────────────────────────────────────────
    build: {
      // 启用 CSS 代码分割：每个异步 chunk 单独提取对应 CSS
      cssCodeSplit: true,
      // 生产构建启用 sourcemap（false = 更小体积，true = 便于生产调试）
      sourcemap: false,
      // chunk 大小警告阈值提升到 800KB（recharts 等库体积较大）
      chunkSizeWarningLimit: 800,
      rollupOptions: {
        output: {
          // 手动分 chunk：将图表库单独打包，不阻塞首屏加载
          manualChunks: {
            // React 核心 - 几乎不更新，长期缓存
            'vendor-react': ['react', 'react-dom', 'react-router-dom'],
            // 图表库 - 体积最大，单独分离
            'vendor-charts': ['recharts'],
            // 图标库
            'vendor-icons': ['lucide-react'],
            // 状态管理 + 工具
            'vendor-utils': ['zustand', 'axios', 'react-hot-toast'],
          },
          // chunk 文件名带内容哈希，确保缓存精确失效
          chunkFileNames: 'assets/[name]-[hash].js',
          entryFileNames: 'assets/[name]-[hash].js',
          assetFileNames: 'assets/[name]-[hash].[ext]',
        },
      },
    },

    // ── 依赖预构建优化 ────────────────────────────────────────────────────
    optimizeDeps: {
      // 强制预构建这些依赖，避免开发服务器首次加载时卡顿
      include: [
        'react',
        'react-dom',
        'react-router-dom',
        'axios',
        'zustand',
        'recharts',
        'lucide-react',
        'react-hot-toast',
      ],
    },

    // Vite options tailored for Tauri development
    clearScreen: false,
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 1421,
          }
        : undefined,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    },
  };
});
