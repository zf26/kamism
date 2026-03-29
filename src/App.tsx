import { BrowserRouter, Routes, Route, Navigate, useLocation } from 'react-router-dom';
import { Toaster } from 'react-hot-toast';
import { useAuthStore } from './stores/auth';
import { applyStoredTheme } from './stores/theme';
import { lazy, Suspense } from 'react';
import Layout from './components/Layout';
import ConfirmDialog from './components/ConfirmDialog';
import Login from './pages/auth/Login';
import Register from './pages/auth/Register';
import ResetPassword from './pages/auth/ResetPassword';

// 立即同步主题，避免首屏闪烁
applyStoredTheme();

// 其余页面懒加载
const AdminDashboard    = lazy(() => import('./pages/admin/Dashboard'));
const Merchants         = lazy(() => import('./pages/admin/Merchants'));
const PlanConfigs       = lazy(() => import('./pages/admin/PlanConfigs'));
const AdminMessages     = lazy(() => import('./pages/admin/Messages'));
const MerchantDashboard = lazy(() => import('./pages/merchant/Dashboard'));
const Apps              = lazy(() => import('./pages/merchant/Apps'));
const Cards             = lazy(() => import('./pages/merchant/Cards'));
const Activations       = lazy(() => import('./pages/merchant/Activations'));
const Settings          = lazy(() => import('./pages/merchant/Settings'));
const MerchantMessages  = lazy(() => import('./pages/merchant/Messages'));
const Blacklist         = lazy(() => import('./pages/merchant/Blacklist'));
const Agents            = lazy(() => import('./pages/merchant/Agents'));
const ApiDocs           = lazy(() => import('./pages/merchant/ApiDocs'));

const PageFallback = () => (
  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100vh', background: 'var(--bg)' }}>
    <span className="spinner" />
  </div>
);

function RequireAuth({ children, role }: { children: React.ReactNode; role?: string }) {
  const { token, role: userRole } = useAuthStore();
  if (!token) return <Navigate to="/login" replace />;
  if (role && userRole !== role) {
    return <Navigate to={userRole === 'admin' ? '/admin/dashboard' : '/dashboard'} replace />;
  }
  return <>{children}</>;
}

// key={pageKey} 让每次路由切换时页面组件完全卸载重挂载
// 确保 loading=true + data=[] 初始状态，骨架屏正确显示，不复用旧状态
function AppRoutes() {
  const { role } = useAuthStore();
  const location = useLocation();
  const pageKey = location.pathname;

  return (
    <>
      <Toaster
        position="top-right"
        toastOptions={{
          style: {
            background: 'var(--bg-card)',
            color: 'var(--text)',
            border: '1px solid var(--border-light)',
            fontFamily: 'var(--sans)',
            fontSize: '13px',
          },
          success: { iconTheme: { primary: 'var(--success)', secondary: 'var(--bg-card)' } },
          error:   { iconTheme: { primary: 'var(--danger)',  secondary: 'var(--bg-card)' } },
        }}
      />
      <ConfirmDialog />
      <Suspense fallback={<PageFallback />}>
        <Routes>
          {/* Public */}
          <Route path="/login"          element={<Login />} />
          <Route path="/register"       element={<Register />} />
          <Route path="/reset-password" element={<ResetPassword />} />

          {/* Admin */}
          <Route path="/admin/dashboard"    element={<RequireAuth role="admin"><Layout><AdminDashboard    key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/admin/merchants"    element={<RequireAuth role="admin"><Layout><Merchants         key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/admin/plan-configs" element={<RequireAuth role="admin"><Layout><PlanConfigs       key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/admin/messages"     element={<RequireAuth role="admin"><Layout><AdminMessages     key={pageKey} /></Layout></RequireAuth>} />

          {/* Merchant */}
          <Route path="/dashboard"   element={<RequireAuth role="merchant"><Layout><MerchantDashboard key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/apps"        element={<RequireAuth role="merchant"><Layout><Apps              key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/cards"       element={<RequireAuth role="merchant"><Layout><Cards             key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/activations" element={<RequireAuth role="merchant"><Layout><Activations       key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/settings"    element={<RequireAuth role="merchant"><Layout><Settings          key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/messages"    element={<RequireAuth role="merchant"><Layout><MerchantMessages  key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/blacklist"   element={<RequireAuth role="merchant"><Layout><Blacklist          key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/agents"      element={<RequireAuth role="merchant"><Layout><Agents             key={pageKey} /></Layout></RequireAuth>} />
          <Route path="/api-docs"    element={<RequireAuth role="merchant"><Layout><ApiDocs            key={pageKey} /></Layout></RequireAuth>} />

          {/* Default */}
          <Route path="/"  element={<Navigate to={role === 'admin' ? '/admin/dashboard' : role === 'merchant' ? '/dashboard' : '/login'} replace />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </Suspense>
    </>
  );
}

export default function App() {
  return (
    <BrowserRouter basename="/">
      <AppRoutes />
    </BrowserRouter>
  );
}
