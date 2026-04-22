import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Copy, ExternalLink, Loader2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { DeviceFlowStart, DeviceFlowResult } from '../types/auth';

interface LoginProps {
  onLoginSuccess: () => void;
}

type LoginState = 'idle' | 'loading' | 'code_ready' | 'polling' | 'error';

function Login({ onLoginSuccess }: LoginProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<LoginState>('idle');
  const [userCode, setUserCode] = useState('');
  const [verificationUri, setVerificationUri] = useState('');
  const [error, setError] = useState('');
  const [copied, setCopied] = useState(false);

  const startLogin = async () => {
    setState('loading');
    setError('');

    try {
      const flow = await invoke<DeviceFlowStart>('start_device_flow');
      setUserCode(flow.userCode);
      setVerificationUri(flow.verificationUri);
      setState('code_ready');

      // Auto-start polling after showing code
      setState('polling');
      await invoke<DeviceFlowResult>('poll_device_flow', {
        deviceCode: flow.deviceCode,
        interval: flow.interval,
        expiresIn: flow.expiresIn,
      });

      // Login success — init services then navigate
      await invoke('init_services');
      onLoginSuccess();
    } catch (e: any) {
      setState('error');
      setError(typeof e === 'string' ? e : e.message || 'Login failed');
    }
  };

  const handleCopy = async () => {
    await navigator.clipboard.writeText(userCode);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleOpenGithub = () => {
    // Use Tauri opener plugin to open URL in system browser
    invoke('plugin:opener|open_url', { url: verificationUri }).catch(() => {
      // Fallback: try window.open
      window.open(verificationUri, '_blank');
    });
  };

  return (
    <div className="h-screen flex flex-col bg-[#FAFBFC] dark:bg-base-300">
      {/* Window drag region */}
      <div
        className="fixed top-0 left-0 right-0 h-9"
        style={{ zIndex: 9999, backgroundColor: 'rgba(0,0,0,0.001)', cursor: 'default', userSelect: 'none', WebkitUserSelect: 'none' }}
        data-tauri-drag-region
        onMouseDown={() => getCurrentWindow().startDragging()}
      />

      <div className="flex-1 flex items-center justify-center pt-9">
        <div className="w-full max-w-sm mx-auto px-6 text-center space-y-8">
          {/* Logo */}
          <div className="space-y-3">
            <div className="relative inline-block group">
              <div className="absolute inset-0 bg-orange-500/20 rounded-3xl blur-xl group-hover:blur-2xl transition-all duration-500" />
              <img
                src="/icon.png"
                alt="Logo"
                className="relative w-20 h-20 rounded-3xl shadow-2xl object-cover bg-white dark:bg-black transform group-hover:scale-105 transition-all duration-500 rotate-3 group-hover:rotate-6"
              />
            </div>
            <h1 className="text-2xl font-bold text-gray-900 dark:text-base-content">
              Claude Ultra
            </h1>
          </div>

          {/* Login states */}
          {state === 'idle' && (
            <button
              onClick={startLogin}
              className="w-full py-3 bg-gray-900 dark:bg-white text-white dark:text-gray-900 font-medium rounded-lg hover:opacity-90 transition-opacity flex items-center justify-center gap-2"
            >
              <svg className="w-5 h-5" fill="currentColor" viewBox="0 0 24 24">
                <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z" />
              </svg>
              {t('auth.sign_in_github', 'Sign in with GitHub')}
            </button>
          )}

          {state === 'loading' && (
            <div className="flex items-center justify-center gap-2 text-gray-500 dark:text-gray-400">
              <Loader2 className="w-5 h-5 animate-spin" />
              <span>{t('common.loading', 'Loading...')}</span>
            </div>
          )}

          {(state === 'code_ready' || state === 'polling') && (
            <div className="space-y-4">
              <p className="text-sm text-gray-600 dark:text-gray-400">
                {t('auth.enter_code', 'Enter this code on GitHub:')}
              </p>
              <div className="bg-gray-100 dark:bg-base-200 rounded-xl p-4 border border-gray-200 dark:border-base-100">
                <div className="text-3xl font-mono font-bold tracking-widest text-gray-900 dark:text-base-content">
                  {userCode}
                </div>
              </div>
              <div className="flex gap-2">
                <button
                  onClick={handleCopy}
                  className="flex-1 py-2.5 bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 text-sm font-medium rounded-lg hover:bg-gray-200 dark:hover:bg-base-100 transition-colors flex items-center justify-center gap-1.5"
                >
                  <Copy className="w-4 h-4" />
                  {copied ? t('common.copied', 'Copied!') : t('common.copy', 'Copy')}
                </button>
                <button
                  onClick={handleOpenGithub}
                  className="flex-1 py-2.5 bg-blue-500 text-white text-sm font-medium rounded-lg hover:bg-blue-600 transition-colors flex items-center justify-center gap-1.5"
                >
                  <ExternalLink className="w-4 h-4" />
                  {t('auth.open_github', 'Open GitHub')}
                </button>
              </div>
              {state === 'polling' && (
                <div className="flex items-center justify-center gap-2 text-sm text-gray-500 dark:text-gray-400">
                  <Loader2 className="w-4 h-4 animate-spin" />
                  {t('auth.waiting_auth', 'Waiting for authorization...')}
                </div>
              )}
            </div>
          )}

          {state === 'error' && (
            <div className="space-y-4">
              <div className="bg-red-50 dark:bg-red-900/20 text-red-600 dark:text-red-400 rounded-lg p-3 text-sm">
                {error}
              </div>
              <button
                onClick={startLogin}
                className="w-full py-2.5 bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 font-medium rounded-lg hover:bg-gray-200 dark:hover:bg-base-100 transition-colors"
              >
                {t('auth.try_again', 'Try Again')}
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default Login;
