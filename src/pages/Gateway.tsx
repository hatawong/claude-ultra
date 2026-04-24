/**
 * Gateway page — aligned with AM service config UI
 * Single card: header (status + toggle) + port/timeout/auto-start + LAN + API key + admin password
 * + CLI config sync CollapsibleCard
 */
import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { showToast } from '../components/common/Toast';
import {
  Settings, Power, Copy, Check, RefreshCw,
  Globe, Shield, Key, Terminal, Edit2,
} from 'lucide-react';
import {
  getGatewayConnectionInfo,
  enableLanSharing,
  disableLanSharing,
  regenerateApiKey,
  type GatewayConnectionInfo,
} from '../services/securityService';
import { cn } from '../utils/cn';
import CollapsibleCard from '../components/common/CollapsibleCard';
import JsonHighlight from '../components/common/JsonHighlight';
import { useAuthStore } from '../stores/useAuthStore';
import { useConfigStore } from '../stores/useConfigStore';

export default function Gateway() {
  const { t } = useTranslation();
  const authStatus = useAuthStore((s) => s.authStatus);
  // admin_password is a forward-looking field for Docker/Web deployment (internal
  // feature builds). In the public desktop build it has no auth path wired up, so
  // we hide the UI to avoid misleading users into thinking it protects something.
  const showAdminPassword = authStatus?.mode === 'internal';
  const [info, setInfo] = useState<GatewayConnectionInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [copied, setCopied] = useState<string | null>(null);
  const [actionLoading, setActionLoading] = useState(false);

  // Editing states
  const [isEditingKey, setIsEditingKey] = useState(false);
  const [tempKey, setTempKey] = useState('');
  const [isEditingPassword, setIsEditingPassword] = useState(false);
  const [tempPassword, setTempPassword] = useState('');
  const [transparentError, setTransparentError] = useState<string>('');
  // Vercel states
  const config = useConfigStore((s) => s.config);
  const [isEditingVercel, setIsEditingVercel] = useState(false);
  const [tempVercelKey, setTempVercelKey] = useState('');
  const [vercelTesting, setVercelTesting] = useState(false);
  const [vercelStatus, setVercelStatus] = useState<'unknown' | 'connected' | 'failed'>('unknown');
  // CLI sync states
  const [syncingMode, setSyncingMode] = useState<'proxy' | 'transparent' | 'restore' | null>(null);
  const [currentEnv, setCurrentEnv] = useState<{
    ANTHROPIC_BASE_URL?: string;
    ANTHROPIC_API_KEY?: string;
  }>({});

  const loadCurrentSettings = async () => {
    try {
      const data = await invoke<{ env?: Record<string, string> }>('get_claude_settings');
      setCurrentEnv({
        ANTHROPIC_BASE_URL: data?.env?.ANTHROPIC_BASE_URL,
        ANTHROPIC_API_KEY: data?.env?.ANTHROPIC_API_KEY,
      });
    } catch (e) {
      console.error('Failed to load Claude settings', e);
    }
  };

  const loadInfo = async () => {
    try {
      const data = await getGatewayConnectionInfo();
      setInfo(data);
    } catch (e) {
      console.error('Failed to load gateway info', e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadInfo();
    loadCurrentSettings();
    const interval = setInterval(loadInfo, 5000);
    return () => clearInterval(interval);
  }, []);


  const copyToClipboard = async (text: string, label: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(label);
    setTimeout(() => setCopied(null), 2000);
  };

  const handleToggle = async () => {
    setActionLoading(true);
    try {
      if (info?.running) {
        await invoke('stop_gateway');
      } else {
        await invoke('start_gateway');
      }
      await loadInfo();
    } catch (e) {
      console.error('Failed to toggle gateway', e);
      showToast(t('gateway.error.toggle_failed', 'Failed to toggle gateway: ') + String(e), 'error');
    } finally {
      setActionLoading(false);
    }
  };

  const handleToggleLan = async (enabled: boolean) => {
    setActionLoading(true);
    try {
      if (enabled) {
        await enableLanSharing();
      } else {
        await disableLanSharing();
      }
      if (info?.running) {
        await invoke('stop_gateway');
        await invoke('start_gateway');
      }
      await loadInfo();
    } catch (e) {
      console.error('Failed to toggle LAN', e);
      showToast(t('gateway.error.lan_failed', 'Failed to toggle LAN sharing: ') + String(e), 'error');
    } finally {
      setActionLoading(false);
    }
  };

  const updateConfig = async (updates: Record<string, unknown>) => {
    try {
      await invoke('update_gateway_config', { request: updates });
      if ('transparentEnabled' in updates || 'transparentPort' in updates) {
        setTransparentError('');
      }
      await loadInfo();
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e as { message?: string })?.message ?? String(e);
      if ('transparentEnabled' in updates || 'transparentPort' in updates) {
        setTransparentError(msg);
      }
      console.error('Failed to update config', e);
    }
  };

  const handleRegenerateKey = async () => {
    try {
      await regenerateApiKey();
      await loadInfo();
    } catch (e) {
      console.error('Failed to regenerate key', e);
    }
  };

  const handleSaveApiKey = async () => {
    if (tempKey.trim()) {
      await updateConfig({ apiKey: tempKey.trim() });
    }
    setIsEditingKey(false);
  };

  const handleSyncClaude = async () => {
    setSyncingMode('proxy');
    try {
      await invoke('sync_claude_settings', {
        baseUrl: localEndpoint,
        apiKey: info?.apiKey || '',
      });
      await loadCurrentSettings();
    } catch (e) {
      console.error('Failed to sync Claude settings', e);
    } finally {
      setSyncingMode(null);
    }
  };

  const handleSyncClaudeTransparent = async () => {
    const port = info?.transparentPort ?? 9001;
    const baseUrl = `http://localhost:${port}`;
    const message = t(
      'gateway.cli_sync.confirm_transparent',
      'Switch CC CLI to the transparent audit port ({url}) and remove ANTHROPIC_API_KEY from ~/.claude/settings.json (OAuth required for transparent mode). Continue?',
    ).replace('{url}', baseUrl);
    const ok = window.confirm(message);
    if (!ok) return;
    setSyncingMode('transparent');
    try {
      await invoke('sync_claude_settings_transparent', { baseUrl });
      await loadCurrentSettings();
    } catch (e) {
      console.error('Failed to sync Claude settings (transparent)', e);
    } finally {
      setSyncingMode(null);
    }
  };

  const handleRestoreClaude = async () => {
    const message = t(
      'gateway.cli_sync.confirm_restore',
      'Remove ANTHROPIC_BASE_URL and ANTHROPIC_API_KEY from ~/.claude/settings.json. CC CLI will revert to its default upstream with OAuth credentials. Other settings (telemetry / cache flags) are kept. Continue?',
    );
    const ok = window.confirm(message);
    if (!ok) return;
    setSyncingMode('restore');
    try {
      await invoke('restore_claude_settings');
      await loadCurrentSettings();
    } catch (e) {
      console.error('Failed to restore Claude settings', e);
    } finally {
      setSyncingMode(null);
    }
  };

  const handleSavePassword = async () => {
    await updateConfig({ adminPassword: tempPassword });
    setIsEditingPassword(false);
  };

  // Loading state
  if (loading) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="flex flex-col items-center gap-4">
          <RefreshCw size={32} className="animate-spin text-blue-500" />
          <span className="text-sm text-gray-500 dark:text-gray-400">
            {t('common.loading', 'Loading...')}
          </span>
        </div>
      </div>
    );
  }

  const isLan = info?.bindAddress === '0.0.0.0';
  const localEndpoint = `http://localhost:${info?.port}`;
  const lanBaseUrl = info?.lanIp ? `http://${info.lanIp}:${info?.port}` : null;

  // Derive which CLI sync mode is currently active in ~/.claude/settings.json.
  const activeMode: 'proxy' | 'transparent' | 'restored' | 'unknown' = (() => {
    const url = currentEnv.ANTHROPIC_BASE_URL;
    const hasKey = Boolean(currentEnv.ANTHROPIC_API_KEY);
    if (!url && !hasKey) return 'restored';
    const transparentPort = info?.transparentPort ?? 9001;
    if (url === `http://localhost:${transparentPort}`) return 'transparent';
    if (url === localEndpoint && hasKey) return 'proxy';
    return 'unknown';
  })();

  return (
    <div className="h-full w-full overflow-y-auto overflow-x-hidden">
      <div className="p-5 space-y-4 max-w-7xl mx-auto">

        {/* ── Service config (single card) ── */}
        <div className="bg-white dark:bg-base-100 rounded-lg shadow-sm border border-gray-100 dark:border-base-200">
          {/* Header: title + status + button */}
          <div className="px-4 py-2.5 border-b border-gray-100 dark:border-base-200 flex items-center justify-between">
            <div className="flex items-center gap-4">
              <h2 className="text-base font-semibold flex items-center gap-2 text-gray-900 dark:text-base-content">
                <Settings size={18} />
                {t('gateway.title', 'Gateway')}
              </h2>
              <div className="flex items-center gap-2 pl-4 border-l border-gray-200 dark:border-base-300">
                <div className={cn(
                  'w-2 h-2 rounded-full',
                  info?.running ? 'bg-green-500 animate-pulse' : 'bg-gray-400',
                )} />
                <span className={cn(
                  'text-xs font-medium',
                  info?.running ? 'text-green-600 dark:text-green-400' : 'text-gray-500',
                )}>
                  {info?.running
                    ? `${t('gateway.status.running', 'Running')} (${info.activeAccounts} ${t('gateway.accounts', 'accounts')})`
                    : t('gateway.status.stopped', 'Stopped')}
                </span>
              </div>
            </div>
            <button
              onClick={handleToggle}
              disabled={actionLoading}
              className={cn(
                'px-3 py-1 rounded-lg text-xs font-medium transition-colors flex items-center gap-2',
                info?.running
                  ? 'bg-red-50 text-red-600 hover:bg-red-100 border border-red-200 dark:bg-red-900/20 dark:text-red-400 dark:border-red-800 dark:hover:bg-red-900/30'
                  : 'bg-blue-600 hover:bg-blue-700 text-white shadow-sm shadow-blue-500/30',
                actionLoading && 'opacity-50 cursor-not-allowed',
              )}
            >
              <Power size={14} />
              {actionLoading
                ? t('gateway.status.processing', 'Processing...')
                : info?.running
                  ? t('gateway.action.stop', 'Stop')
                  : t('gateway.action.start', 'Start')}
            </button>
          </div>

          <div className="p-4 space-y-4">
            {/* Row 1: Port + Timeout + Auto-start */}
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
              <div>
                <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                  {t('gateway.config.port', 'Port')}
                </label>
                <input
                  type="number"
                  value={info?.port || 9000}
                  onChange={(e) => updateConfig({ port: parseInt(e.target.value) })}
                  min={1024}
                  max={65535}
                  disabled={info?.running}
                  className="w-full px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 text-xs text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500 focus:border-transparent disabled:opacity-50 disabled:cursor-not-allowed"
                />
                <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">
                  {t('gateway.config.port_hint', 'Default 9000. Restart to apply.')}
                </p>
              </div>
              <div>
                <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                  {t('gateway.config.request_timeout', 'Request Timeout')}
                </label>
                <input
                  type="number"
                  value={info?.requestTimeout || 300}
                  onChange={(e) => {
                    const v = parseInt(e.target.value);
                    updateConfig({ requestTimeout: Math.max(30, Math.min(7200, v)) });
                  }}
                  min={30}
                  max={7200}
                  disabled={info?.running}
                  className="w-full px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 text-xs text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500 focus:border-transparent disabled:opacity-50 disabled:cursor-not-allowed"
                />
                <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">
                  {t('gateway.config.request_timeout_hint', 'Default 300s. Range 30-7200s.')}
                </p>
              </div>
              <div className="flex items-center">
                <label className="flex items-center cursor-pointer gap-3">
                  <input
                    type="checkbox"
                    className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500"
                    checked={info?.autoStart ?? true}
                    onChange={(e) => updateConfig({ autoStart: e.target.checked })}
                  />
                  <span className="text-xs font-medium text-gray-900 dark:text-base-content">
                    {t('gateway.config.auto_start', 'Auto-start')}
                  </span>
                </label>
              </div>
            </div>

            {/* Divider + LAN */}
            <div className="border-t border-gray-200 dark:border-base-300 pt-3">
              <div className="flex items-center justify-between">
                <span className="text-xs font-medium text-gray-700 dark:text-gray-300 flex items-center gap-1.5">
                  <Globe size={14} className="text-blue-500" />
                  {t('gateway.lan.title', 'LAN Sharing')}
                </span>
                <div className="flex items-center gap-2">
                  {isLan && (
                    <span className="text-[10px] px-2 py-0.5 rounded-full bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-400 flex items-center gap-1">
                      <Shield size={10} />
                      {t('gateway.lan.whitelist_active', 'Whitelist Active')}
                    </span>
                  )}
                  <input
                    type="checkbox"
                    className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500"
                    checked={isLan}
                    onChange={(e) => handleToggleLan(e.target.checked)}
                    disabled={actionLoading}
                  />
                </div>
              </div>
              <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-1">
                {isLan
                  ? t('gateway.lan.enabled_desc', 'Other devices on your network can connect to this gateway.')
                  : t('gateway.lan.disabled_desc', 'Only this machine can connect. Enable to allow LAN access.')}
              </p>
              {info?.running && (
                <p className="text-[10px] text-blue-600 dark:text-blue-400 mt-1">
                  {t('gateway.lan.restart_hint', 'Changes require restart to take effect.')}
                </p>
              )}
            </div>

            {/* Divider + API Key */}
            <div className="border-t border-gray-200 dark:border-base-300 pt-3">
              <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1 flex items-center gap-1.5">
                <Key size={14} className="text-amber-500" />
                {t('gateway.apikey.title', 'API Key')}
              </label>
              <div className="flex gap-2">
                <input
                  type="text"
                  value={isEditingKey ? tempKey : (info?.apiKey || '')}
                  onChange={(e) => isEditingKey && setTempKey(e.target.value)}
                  readOnly={!isEditingKey}
                  className={cn(
                    'flex-1 px-2.5 py-1.5 border rounded-lg text-xs font-mono',
                    isEditingKey
                      ? 'bg-white dark:bg-base-200 border-blue-300 dark:border-blue-500 text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500'
                      : 'bg-gray-50 dark:bg-base-300 border-gray-300 dark:border-base-200 text-gray-600 dark:text-gray-400',
                  )}
                />
                {isEditingKey ? (
                  <>
                    <button
                      onClick={handleSaveApiKey}
                      className="p-1.5 text-green-500 hover:bg-green-50 dark:hover:bg-green-900/30 rounded-md transition-colors"
                      title={t('common.save', 'Save')}
                    >
                      <Check size={14} />
                    </button>
                    <button
                      onClick={() => setIsEditingKey(false)}
                      className="p-1.5 text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700 rounded-md transition-colors"
                      title={t('common.cancel', 'Cancel')}
                    >
                      ✕
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      onClick={() => { setTempKey(info?.apiKey || ''); setIsEditingKey(true); }}
                      className="p-1.5 text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/30 rounded-md transition-colors"
                      title={t('common.edit', 'Edit')}
                    >
                      <Edit2 size={14} />
                    </button>
                    <button
                      onClick={handleRegenerateKey}
                      className="p-1.5 text-gray-400 hover:text-amber-500 hover:bg-amber-50 dark:hover:bg-amber-900/30 rounded-md transition-colors"
                      title={t('gateway.apikey.regenerate', 'Regenerate')}
                    >
                      <RefreshCw size={14} />
                    </button>
                    <button
                      onClick={() => copyToClipboard(info?.apiKey || '', 'key')}
                      className="p-1.5 text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/30 rounded-md transition-colors"
                      title={t('common.copy', 'Copy')}
                    >
                      {copied === 'key' ? <Check size={14} className="text-green-500" /> : <Copy size={14} />}
                    </button>
                  </>
                )}
              </div>
              <p className="mt-1 text-[10px] text-amber-600 dark:text-amber-500">
                {t('gateway.apikey.warning', 'Keep your API key safe. Do not share it.')}
              </p>
            </div>

            {/* Divider + Vercel AI Gateway */}
            <div className="border-t border-gray-200 dark:border-base-300 pt-3">
              <label className="flex items-center gap-1.5 text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                <Globe size={14} className="text-blue-500" />
                {t('gateway.vercel.title', 'Vercel AI Gateway')}
              </label>
              <div className="flex gap-2">
                <input
                  type={isEditingVercel ? 'text' : 'password'}
                  value={isEditingVercel ? tempVercelKey : (config?.gateway?.vercel_api_key || '')}
                  onChange={(e) => isEditingVercel && setTempVercelKey(e.target.value)}
                  readOnly={!isEditingVercel}
                  placeholder={config?.gateway?.vercel_api_key ? '••••••••' : t('gateway.vercel.not_configured', 'Not Configured')}
                  className={cn(
                    'flex-1 px-2.5 py-1.5 border rounded-lg text-xs font-mono',
                    isEditingVercel
                      ? 'bg-white dark:bg-base-200 border-blue-300 dark:border-blue-500 text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500'
                      : 'bg-gray-50 dark:bg-base-300 border-gray-300 dark:border-base-200 text-gray-600 dark:text-gray-400',
                  )}
                />
                {isEditingVercel ? (
                  <>
                    <button
                      onClick={async () => {
                        try {
                          await invoke('update_gateway_config', { request: { vercelApiKey: tempVercelKey } });
                          setIsEditingVercel(false);
                          await useConfigStore.getState().loadConfig();
                          // Restart gateway to pick up new vercel_api_key in AppState
                          if (info?.running) {
                            await invoke('stop_gateway');
                            await invoke('start_gateway');
                            await loadInfo();
                          }
                          showToast(t('common.saved', 'Saved'), 'success');
                        } catch (e) { showToast(String(e), 'error'); }
                      }}
                      className="p-1.5 text-green-500 hover:bg-green-50 dark:hover:bg-green-900/30 rounded-md transition-colors"
                      title={t('common.save', 'Save')}
                    >
                      <Check size={14} />
                    </button>
                    <button
                      onClick={() => setIsEditingVercel(false)}
                      className="p-1.5 text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700 rounded-md transition-colors"
                      title={t('common.cancel', 'Cancel')}
                    >
                      ✕
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      onClick={() => { setTempVercelKey(config?.gateway?.vercel_api_key || ''); setIsEditingVercel(true); }}
                      className="p-1.5 text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/30 rounded-md transition-colors"
                      title={t('common.edit', 'Edit')}
                    >
                      <Edit2 size={14} />
                    </button>
                    <button
                      disabled={vercelTesting}
                      onClick={async () => {
                        setVercelTesting(true);
                        try {
                          await invoke('test_vercel_connection');
                          setVercelStatus('connected');
                        } catch {
                          setVercelStatus('failed');
                        } finally {
                          setVercelTesting(false);
                        }
                      }}
                      className="px-2 py-1 text-[10px] font-medium rounded-md border border-gray-200 dark:border-base-300 text-gray-600 dark:text-gray-400 hover:bg-gray-50 dark:hover:bg-base-200 disabled:opacity-40 transition-colors"
                    >
                      {vercelTesting ? '...' : t('gateway.vercel.test', 'Test')}
                    </button>
                  </>
                )}
              </div>
              <div className="mt-1 flex items-center gap-1.5">
                {config?.gateway?.vercel_api_key ? (
                  <span className={cn('text-[10px]', vercelStatus === 'connected' ? 'text-green-600' : vercelStatus === 'failed' ? 'text-red-500' : 'text-gray-400')}>
                    {vercelStatus === 'connected' ? `✅ ${t('gateway.vercel.connected', 'Connected')}` :
                     vercelStatus === 'failed' ? `❌ ${t('gateway.vercel.test_failed', 'Failed')}` :
                     t('gateway.vercel.not_tested', 'Not tested')}
                  </span>
                ) : (
                  <span className="text-[10px] text-gray-400">{t('gateway.vercel.not_configured', 'Not Configured')}</span>
                )}
              </div>
            </div>

            {/* Divider + Admin Password (internal builds only — no auth path in public) */}
            {showAdminPassword && (
            <div className="border-t border-gray-200 dark:border-base-300 pt-3">
              <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                {t('gateway.admin_password.title', 'Web UI Admin Password')}
              </label>
              <div className="flex gap-2">
                <input
                  type={isEditingPassword ? 'text' : 'password'}
                  value={isEditingPassword ? tempPassword : (info?.adminPassword || '')}
                  onChange={(e) => isEditingPassword && setTempPassword(e.target.value)}
                  readOnly={!isEditingPassword}
                  placeholder={isEditingPassword ? '' : (info?.adminPassword ? '••••••••' : t('gateway.admin_password.placeholder', '(Same as API Key)'))}
                  className={cn(
                    'flex-1 px-2.5 py-1.5 border rounded-lg text-xs font-mono',
                    isEditingPassword
                      ? 'bg-white dark:bg-base-200 border-blue-300 dark:border-blue-500 text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500'
                      : 'bg-gray-50 dark:bg-base-300 border-gray-300 dark:border-base-200 text-gray-600 dark:text-gray-400',
                  )}
                />
                {isEditingPassword ? (
                  <>
                    <button
                      onClick={handleSavePassword}
                      className="p-1.5 text-green-500 hover:bg-green-50 dark:hover:bg-green-900/30 rounded-md transition-colors"
                      title={t('common.save', 'Save')}
                    >
                      <Check size={14} />
                    </button>
                    <button
                      onClick={() => setIsEditingPassword(false)}
                      className="p-1.5 text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700 rounded-md transition-colors"
                      title={t('common.cancel', 'Cancel')}
                    >
                      ✕
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      onClick={() => { setTempPassword(info?.adminPassword || ''); setIsEditingPassword(true); }}
                      className="p-1.5 text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/30 rounded-md transition-colors"
                      title={t('common.edit', 'Edit')}
                    >
                      <Edit2 size={14} />
                    </button>
                    <button
                      onClick={() => copyToClipboard(info?.adminPassword || info?.apiKey || '', 'pwd')}
                      className="p-1.5 text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/30 rounded-md transition-colors"
                      title={t('common.copy', 'Copy')}
                    >
                      {copied === 'pwd' ? <Check size={14} className="text-green-500" /> : <Copy size={14} />}
                    </button>
                  </>
                )}
              </div>
              <p className="mt-1 text-[10px] text-gray-500 dark:text-gray-400">
                {t('gateway.admin_password.hint', 'For Docker/Web deployment. Set a separate login password to improve API key security.')}
              </p>
            </div>
            )}

            {/* Transparent audit port (internal builds only) */}
            {authStatus?.mode === 'internal' && (
            <div className="border-t border-gray-200 dark:border-base-300 pt-3">
              <div className="flex items-center justify-between mb-1">
                <label className="block text-xs font-medium text-gray-700 dark:text-gray-300">
                  {t('gateway.transparent.title', 'Transparent Audit Port')}
                  <span className="ml-1.5 inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-medium bg-amber-100 text-amber-800 dark:bg-amber-900/40 dark:text-amber-200">
                    {t('gateway.transparent.internal_badge', 'Internal Only')}
                  </span>
                  {info?.transparentEnabled && (
                    <span
                      className={cn(
                        'ml-1.5 inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-medium',
                        info?.transparentRunning
                          ? 'bg-green-100 text-green-800 dark:bg-green-900/40 dark:text-green-200'
                          : 'bg-red-100 text-red-800 dark:bg-red-900/40 dark:text-red-200',
                      )}
                    >
                      {info?.transparentRunning
                        ? t('gateway.transparent.status_running', 'Running')
                        : t('gateway.transparent.status_stopped', 'Stopped')}
                    </span>
                  )}
                </label>
                <label className="inline-flex items-center gap-1.5 cursor-pointer">
                  <input
                    type="checkbox"
                    className="toggle toggle-xs toggle-primary"
                    checked={info?.transparentEnabled ?? false}
                    disabled={info?.running}
                    onChange={(e) => updateConfig({ transparentEnabled: e.target.checked })}
                  />
                </label>
              </div>
              <p className="text-[10px] text-gray-500 dark:text-gray-400 mb-2">
                {t('gateway.transparent.description', 'When enabled, all requests on this port are passed through to Anthropic as-is, logged but unmodified.')}
              </p>
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-[10px] text-gray-500 dark:text-gray-400 mb-0.5">
                    {t('gateway.transparent.port_label', 'Port')}
                  </label>
                  <input
                    type="number"
                    min={1024}
                    max={65535}
                    value={info?.transparentPort ?? 9001}
                    disabled={info?.running}
                    onChange={(e) => {
                      const v = parseInt(e.target.value, 10);
                      if (!Number.isNaN(v)) updateConfig({ transparentPort: v });
                    }}
                    className="w-full px-2 py-1 border border-gray-300 dark:border-base-300 rounded text-xs font-mono bg-white dark:bg-base-200 disabled:bg-gray-50 dark:disabled:bg-base-300"
                  />
                </div>
                <div>
                  <label className="block text-[10px] text-gray-500 dark:text-gray-400 mb-0.5">
                    {t('gateway.transparent.bound_to', 'Bound to')}
                  </label>
                  <div className="px-2 py-1 border border-gray-200 dark:border-base-300 rounded text-xs font-mono bg-gray-50 dark:bg-base-300 text-gray-600 dark:text-gray-400">
                    127.0.0.1
                  </div>
                </div>
              </div>
              <p className="mt-1 text-[10px] text-gray-500 dark:text-gray-400">
                {t('gateway.transparent.port_hint', 'Localhost only. Restart gateway to apply.')}
              </p>
              {transparentError && (
                <p className="mt-1 text-[10px] text-red-600 dark:text-red-400 font-mono">
                  {transparentError}
                </p>
              )}
            </div>
            )}
          </div>
        </div>

        {/* ── CLI config sync ── */}
        <CollapsibleCard
          title={t('gateway.cli_sync.title', 'CLI Config Sync')}
          icon={<Terminal size={18} className="text-gray-500" />}
        >
          <div className="space-y-4">
            <p className="text-xs text-gray-500 dark:text-gray-400">
              {t('gateway.cli_sync.desc', 'Quickly sync API endpoint and key to your local Claude Code CLI.')}
            </p>

            {/* JSON snippet for settings.json */}
            <div>
              <div className="flex items-center justify-between mb-2">
                <span className="text-xs font-medium text-gray-700 dark:text-gray-300">
                  {t('gateway.cli_sync.json_snippet', 'settings.json env snippet')}
                </span>
                <button
                  onClick={() => copyToClipboard(
                    `"env": {\n  "ANTHROPIC_BASE_URL": "${localEndpoint}",\n  "ANTHROPIC_API_KEY": "${info?.apiKey}",\n  "DISABLE_TELEMETRY": "1",\n  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",\n  "ENABLE_PROMPT_CACHING_1H": "1"\n}`,
                    'json',
                  )}
                  className="text-xs text-gray-400 hover:text-blue-500 transition-colors flex items-center gap-1"
                >
                  {copied === 'json' ? <Check size={12} className="text-green-500" /> : <Copy size={12} />}
                  {copied === 'json' ? t('common.copied', 'Copied') : t('common.copy_all', 'Copy All')}
                </button>
              </div>
              <pre className="bg-gray-800/50 dark:bg-gray-900/80 rounded-lg p-3 text-[11px] font-mono text-gray-300 overflow-x-auto select-text leading-relaxed">
                <JsonHighlight json={`"env": {\n  "ANTHROPIC_BASE_URL": "${localEndpoint}",\n  "ANTHROPIC_API_KEY": "${info?.apiKey}",\n  "DISABLE_TELEMETRY": "1",\n  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",\n  "ENABLE_PROMPT_CACHING_1H": "1"\n}`} />
              </pre>
              <p className="mt-1 text-[10px] text-gray-500 dark:text-gray-400">
                {t('gateway.cli_sync.json_hint', 'Paste into the "env" section of ~/.claude/settings.json')}
              </p>
            </div>

            {/* Auto sync button */}
            <div className="flex items-center justify-between p-3 bg-gray-50 dark:bg-base-200 rounded-lg">
              <div>
                <span className="text-xs font-medium text-gray-700 dark:text-gray-300">
                  {t('gateway.cli_sync.auto_sync', 'Auto Sync to Claude Code')}
                </span>
                <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-0.5">
                  {t('gateway.cli_sync.auto_sync_desc', 'Write env vars directly into ~/.claude/settings.json (preserves existing settings)')}
                </p>
              </div>
              {activeMode === 'proxy' ? (
                <span className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-green-50 text-green-600 dark:bg-green-900/20 dark:text-green-400">
                  <Check size={14} /> {t('gateway.cli_sync.synced', 'Synced')}
                </span>
              ) : (
                <button
                  onClick={handleSyncClaude}
                  disabled={syncingMode !== null}
                  className={cn(
                    'px-3 py-1.5 rounded-lg text-xs font-medium transition-colors flex items-center gap-1.5',
                    'bg-blue-600 hover:bg-blue-700 text-white shadow-sm shadow-blue-500/30',
                    syncingMode !== null && 'opacity-50 cursor-not-allowed',
                  )}
                >
                  <RefreshCw size={14} className={syncingMode === 'proxy' ? 'animate-spin' : ''} />
                  {t('gateway.cli_sync.sync_now', 'Sync Now')}
                </button>
              )}
            </div>

            {/* LAN JSON snippet (if enabled) */}
            {isLan && lanBaseUrl && (
              <div>
                <div className="flex items-center justify-between mb-2">
                  <span className="text-xs font-medium text-gray-700 dark:text-gray-300 flex items-center gap-1.5">
                    <Globe size={12} className="text-blue-400" />
                    {t('gateway.cli_sync.lan_snippet', 'LAN settings.json env snippet')}
                    <span className="text-[10px] text-gray-400">({info?.lanIp})</span>
                  </span>
                  <button
                    onClick={() => copyToClipboard(
                      `"env": {\n  "ANTHROPIC_BASE_URL": "${lanBaseUrl}",\n  "ANTHROPIC_API_KEY": "${info?.apiKey}",\n  "DISABLE_TELEMETRY": "1",\n  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",\n  "ENABLE_PROMPT_CACHING_1H": "1"\n}`,
                      'lan-json',
                    )}
                    className="text-xs text-gray-400 hover:text-blue-500 transition-colors flex items-center gap-1"
                  >
                    {copied === 'lan-json' ? <Check size={12} className="text-green-500" /> : <Copy size={12} />}
                    {copied === 'lan-json' ? t('common.copied', 'Copied') : t('common.copy_all', 'Copy All')}
                  </button>
                </div>
                <pre className="bg-blue-900/30 dark:bg-blue-900/20 rounded-lg p-3 text-[11px] font-mono text-blue-200 overflow-x-auto select-text leading-relaxed">
                  <JsonHighlight json={`"env": {\n  "ANTHROPIC_BASE_URL": "${lanBaseUrl}",\n  "ANTHROPIC_API_KEY": "${info?.apiKey}",\n  "DISABLE_TELEMETRY": "1",\n  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",\n  "ENABLE_PROMPT_CACHING_1H": "1"\n}`} />
                </pre>
              </div>
            )}

            {/* Transparent audit mode sync (internal builds + transparent enabled) */}
            {authStatus?.mode === 'internal' && info?.transparentEnabled && (
              <div className="border-t border-gray-200 dark:border-base-300 pt-3">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-xs font-medium text-gray-700 dark:text-gray-300 flex items-center gap-1.5">
                    {t('gateway.cli_sync.transparent_snippet', 'Transparent audit settings.json env snippet')}
                    <span className="ml-1.5 inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-medium bg-amber-100 text-amber-800 dark:bg-amber-900/40 dark:text-amber-200">
                      {t('gateway.transparent.internal_badge', 'Internal Only')}
                    </span>
                  </span>
                  <button
                    onClick={() => {
                      const port = info?.transparentPort ?? 9001;
                      copyToClipboard(
                        `"env": {\n  "ANTHROPIC_BASE_URL": "http://localhost:${port}",\n  "DISABLE_TELEMETRY": "1",\n  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",\n  "ENABLE_PROMPT_CACHING_1H": "1"\n}`,
                        'transparent-json',
                      );
                    }}
                    className="text-xs text-gray-400 hover:text-blue-500 transition-colors flex items-center gap-1"
                  >
                    {copied === 'transparent-json' ? <Check size={12} className="text-green-500" /> : <Copy size={12} />}
                    {copied === 'transparent-json' ? t('common.copied', 'Copied') : t('common.copy_all', 'Copy All')}
                  </button>
                </div>
                <pre className="bg-amber-900/20 dark:bg-amber-900/15 rounded-lg p-3 text-[11px] font-mono text-amber-200 overflow-x-auto select-text leading-relaxed">
                  <JsonHighlight json={`"env": {\n  "ANTHROPIC_BASE_URL": "http://localhost:${info?.transparentPort ?? 9001}",\n  "DISABLE_TELEMETRY": "1",\n  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",\n  "ENABLE_PROMPT_CACHING_1H": "1"\n}`} />
                </pre>
                <p className="mt-1 text-[10px] text-gray-500 dark:text-gray-400">
                  {t('gateway.cli_sync.transparent_hint', 'No ANTHROPIC_API_KEY: transparent mode passes through OAuth credentials to the upstream as-is.')}
                </p>
                <div className="flex items-center justify-between mt-3 p-3 bg-amber-50 dark:bg-amber-900/10 rounded-lg">
                  <div>
                    <span className="text-xs font-medium text-gray-700 dark:text-gray-300">
                      {t('gateway.cli_sync.auto_sync_transparent', 'Auto Sync to Transparent Mode')}
                    </span>
                    <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-0.5">
                      {t('gateway.cli_sync.auto_sync_transparent_desc', 'Switch CC CLI to the transparent audit port and remove ANTHROPIC_API_KEY (OAuth fallback).')}
                    </p>
                  </div>
                  {activeMode === 'transparent' ? (
                    <span className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-green-50 text-green-600 dark:bg-green-900/20 dark:text-green-400">
                      <Check size={14} /> {t('gateway.cli_sync.synced', 'Synced')}
                    </span>
                  ) : (
                    <button
                      onClick={handleSyncClaudeTransparent}
                      disabled={syncingMode !== null}
                      className={cn(
                        'px-3 py-1.5 rounded-lg text-xs font-medium transition-colors flex items-center gap-1.5',
                        'bg-amber-600 hover:bg-amber-700 text-white shadow-sm shadow-amber-500/30',
                        syncingMode !== null && 'opacity-50 cursor-not-allowed',
                      )}
                    >
                      <RefreshCw size={14} className={syncingMode === 'transparent' ? 'animate-spin' : ''} />
                      {t('gateway.cli_sync.sync_now', 'Sync Now')}
                    </button>
                  )}
                </div>
              </div>
            )}

            {/* Restore: remove gateway env (BASE_URL + API_KEY) from settings.json */}
            <div className="border-t border-gray-200 dark:border-base-300 pt-3">
              <div className="flex items-center justify-between p-3 bg-gray-50 dark:bg-base-200 rounded-lg">
                <div>
                  <span className="text-xs font-medium text-gray-700 dark:text-gray-300">
                    {t('gateway.cli_sync.restore_title', 'Restore Default Settings')}
                  </span>
                  <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-0.5">
                    {t('gateway.cli_sync.restore_desc', 'Remove ANTHROPIC_BASE_URL and ANTHROPIC_API_KEY from ~/.claude/settings.json (CC CLI reverts to default upstream).')}
                  </p>
                </div>
                {activeMode === 'restored' ? (
                  <span className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-green-50 text-green-600 dark:bg-green-900/20 dark:text-green-400">
                    <Check size={14} /> {t('gateway.cli_sync.restored', 'Restored')}
                  </span>
                ) : (
                  <button
                    onClick={handleRestoreClaude}
                    disabled={syncingMode !== null}
                    className={cn(
                      'px-3 py-1.5 rounded-lg text-xs font-medium transition-colors flex items-center gap-1.5',
                      'bg-gray-600 hover:bg-gray-700 text-white shadow-sm shadow-gray-500/30',
                      syncingMode !== null && 'opacity-50 cursor-not-allowed',
                    )}
                  >
                    <RefreshCw size={14} className={syncingMode === 'restore' ? 'animate-spin' : ''} />
                    {t('gateway.cli_sync.restore_now', 'Restore Default')}
                  </button>
                )}
              </div>
            </div>
          </div>
        </CollapsibleCard>

      </div>
    </div>
  );
}
