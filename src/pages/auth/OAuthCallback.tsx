import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuthStore } from '../../stores/auth';
import toast from 'react-hot-toast';

export default function OAuthCallback() {
  const navigate = useNavigate();
  const { setAuth } = useAuthStore();
  const [status, setStatus] = useState<'loading' | 'error'>('loading');
  const handledRef = useRef(false);

  useEffect(() => {
    if (handledRef.current) return;
    handledRef.current = true;

    const handleCallback = async () => {
      const params = new URLSearchParams(window.location.search);
      const token = params.get('token');
      const refresh = params.get('refresh');
      const role = params.get('role');
      const userStr = params.get('user');
      const error = params.get('error');
      if (error) {
        const errorMessages: Record<string, string> = {
          access_denied: '您取消了 GitHub 授权',
          csrf: '授权验证失败，请重试',
          token: '获取访问令牌失败',
          userinfo: '获取用户信息失败',
          noemail: '无法获取您的邮箱地址，请确保 GitHub 账号已验证邮箱',
          user: '处理用户信息失败',
        };
        toast.error(errorMessages[error] || 'GitHub 登录失败');
        setStatus('error');
        setTimeout(() => navigate('/login'), 2000);
        return;
      }

      if (!token || !refresh || !role || !userStr) {
        toast.error('OAuth 回调参数不完整');
        setStatus('error');
        setTimeout(() => navigate('/login'), 2000);
        return;
      }

      try {
        const user = JSON.parse(decodeURIComponent(userStr));
        setAuth(token, refresh, role as 'admin' | 'merchant', user);
        toast.success('登录成功');

        const cleanUrl = window.location.pathname;
        window.history.replaceState({}, '', cleanUrl);

        navigate(role === 'admin' ? '/admin/dashboard' : '/dashboard');
      } catch {
        toast.error('用户信息解析失败');
        setStatus('error');
        setTimeout(() => navigate('/login'), 2000);
      }
    };

    handleCallback();
  }, [navigate, setAuth]);

  return (
    <div style={{
      minHeight: '100vh',
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      flexDirection: 'column',
      gap: 16,
    }}>
      {status === 'loading' ? (
        <>
          <span className="spinner" style={{ width: 40, height: 40 }} />
          <p style={{ color: 'var(--text-muted)' }}>正在处理 GitHub 登录...</p>
        </>
      ) : (
        <>
          <p style={{ color: 'var(--danger)' }}>授权失败，正在跳转...</p>
        </>
      )}
    </div>
  );
}
