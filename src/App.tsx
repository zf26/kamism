import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { Toaster } from 'react-hot-toast';
import { useAuthStore } from './stores/auth';
import Layout from './components/Layout';

// Auth pages
import Login from './pages/auth/Login';
import Register from './pages/auth/Register';

// Admin pages
import AdminDashboard from './pages/admin/Dashboard';
import Merchants from './pages/admin/Merchants';

// Merchant pages
import MerchantDashboard from './pages/merchant/Dashboard';
import Apps from './pages/merchant/Apps';
import Cards from './pages/merchant/Cards';
import Activations from './pages/merchant/Activations';
import Settings from './pages/merchant/Settings';

function RequireAuth({ children, role }: { children: React.ReactNode; role?: string }) {
  const { token, role: userRole } = useAuthStore();
  if (!token) return <Navigate to="/login" replace />;
  if (role && userRole !== role) {
    return <Navigate to={userRole === 'admin' ? '/admin/dashboard' : '/dashboard'} replace />;
  }
  return <>{children}</>;
}

export default function App() {
  const { role } = useAuthStore();

  return (
    <BrowserRouter>
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
          error: { iconTheme: { primary: 'var(--danger)', secondary: 'var(--bg-card)' } },
        }}
      />
      <Routes>
        {/* Public */}
        <Route path="/login" element={<Login />} />
        <Route path="/register" element={<Register />} />

        {/* Admin routes */}
        <Route path="/admin/dashboard" element={
          <RequireAuth role="admin"><Layout><AdminDashboard /></Layout></RequireAuth>
        } />
        <Route path="/admin/merchants" element={
          <RequireAuth role="admin"><Layout><Merchants /></Layout></RequireAuth>
        } />

        {/* Merchant routes */}
        <Route path="/dashboard" element={
          <RequireAuth role="merchant"><Layout><MerchantDashboard /></Layout></RequireAuth>
        } />
        <Route path="/apps" element={
          <RequireAuth role="merchant"><Layout><Apps /></Layout></RequireAuth>
        } />
        <Route path="/cards" element={
          <RequireAuth role="merchant"><Layout><Cards /></Layout></RequireAuth>
        } />
        <Route path="/activations" element={
          <RequireAuth role="merchant"><Layout><Activations /></Layout></RequireAuth>
        } />
        <Route path="/settings" element={
          <RequireAuth role="merchant"><Layout><Settings /></Layout></RequireAuth>
        } />

        {/* Default redirect */}
        <Route path="/" element={
          <Navigate to={role === 'admin' ? '/admin/dashboard' : role === 'merchant' ? '/dashboard' : '/login'} replace />
        } />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
