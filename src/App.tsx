import { useEffect, useRef, useState } from 'react';
import { ConfigProvider, theme } from 'antd';
import { createBrowserRouter, RouterProvider } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useConfigStore } from './stores/useConfigStore';
import { useAccountStore } from './stores/useAccountStore';
import { useAuthStore } from './stores/useAuthStore';
import Layout from './components/layout/Layout';
import Dashboard from './pages/Dashboard';
import Accounts from './pages/Accounts';
import Settings from './pages/Settings';
import TokenStats from './pages/TokenStats';
import Monitor from './pages/Monitor';
import Gateway from './pages/Gateway';
import Security from './pages/Security';
import Login from './pages/Login';
import { ToastContainer } from './components/common/Toast';

const router = createBrowserRouter([
  {
    path: '/',
    element: <Layout />,
    children: [
      { index: true, element: <Dashboard /> },
      { path: 'accounts', element: <Accounts /> },
      { path: 'gateway', element: <Gateway /> },
      { path: 'monitor', element: <Monitor /> },
      { path: 'token-stats', element: <TokenStats /> },
      { path: 'security', element: <Security /> },
      { path: 'settings', element: <Settings /> },
    ],
  },
]);

function App() {
  const { loadConfig } = useConfigStore();
  const { authStatus, loading: authLoading, fetchAuthStatus } = useAuthStore();
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [windowShown, setWindowShown] = useState(false);
  const [servicesReady, setServicesReady] = useState(false);

  useEffect(() => {
    loadConfig();
    fetchAuthStatus();
  }, [loadConfig, fetchAuthStatus]);

  // When auth status resolves to authenticated, ensure services are initialized.
  // Covers both JWT restore (no Login page) and internal mode (auto-init in Rust).
  // init_services is idempotent — safe to call even if Rust already started them.
  useEffect(() => {
    if (!authLoading && authStatus) {
      const isAuth = authStatus.mode === 'internal' || authStatus.status === 'logged_in';
      if (isAuth && !servicesReady) {
        invoke('init_services')
          .then(() => setServicesReady(true))
          .catch((e: unknown) => console.error('init_services failed:', e));
      }
      // Reset on logout so re-login triggers init_services again
      if (!isAuth && servicesReady) {
        setServicesReady(false);
      }
    }
  }, [authLoading, authStatus, servicesReady]);

  // Show window once auth status is known (Login or Layout will handle the rest)
  useEffect(() => {
    if (!authLoading && authStatus && !windowShown) {
      getCurrentWindow().show().catch(console.error);
      setWindowShown(true);
    }
  }, [authLoading, authStatus, windowShown]);

  // Listen for account changes from backend (created, disabled, deleted, quota, etc.)
  useEffect(() => {
    const unlisten = listen('account://changed', () => {
      clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        useAccountStore.getState().fetchAccounts();
      }, 300);
    });
    return () => {
      unlisten.then((fn) => fn());
      clearTimeout(debounceRef.current);
    };
  }, []);

  // Determine if user should see main app or login page
  const isAuthenticated = authStatus &&
    (authStatus.mode === 'internal' || authStatus.status === 'logged_in');

  return (
    <ConfigProvider
      theme={{
        algorithm: theme.darkAlgorithm,
        token: {
          colorPrimary: '#3b82f6',
          colorBgContainer: '#1d232a',
          colorBgElevated: '#191e24',
          colorBorder: '#2a323c',
          colorText: '#A6ADBB',
          colorTextSecondary: '#6b7280',
          borderRadius: 8,
        },
      }}
    >
      {authLoading ? null : isAuthenticated ? (
        <RouterProvider router={router} />
      ) : (
        <Login onLoginSuccess={() => fetchAuthStatus()} />
      )}
      <ToastContainer />
    </ConfigProvider>
  );
}

export default App;
