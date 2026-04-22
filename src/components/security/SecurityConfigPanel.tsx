import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Shield, ShieldCheck } from 'lucide-react';
import { getSecurityConfig, updateSecurityConfig, type SecurityConfig } from '../../services/securityService';
import { useAuthStore } from '../../stores/useAuthStore';

interface Props {
  onSaveRef?: React.MutableRefObject<(() => void) | null>;
}

export const SecurityConfigPanel: React.FC<Props> = ({ onSaveRef }) => {
  const { t } = useTranslation();
  const authStatus = useAuthStore((s) => s.authStatus);
  // Auto Ban and Log Retention are configurable in the UI but have no runtime
  // enforcement path in the public build (no failure counter, no blacklist
  // expiry cleaner, no periodic log trim). Showing them would mislead users into
  // thinking those protections are active. Reveal only for internal builds,
  // where the full server-side deployment would wire them up.
  const showEnforcementControls = authStatus?.mode === 'internal';
  const [config, setConfig] = useState<SecurityConfig | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => { loadConfig(); }, []);

  const loadConfig = async () => {
    setLoading(true);
    try {
      setConfig(await getSecurityConfig());
    } catch (e) {
      console.error('Failed to load security config', e);
    } finally {
      setLoading(false);
    }
  };

  const handleSave = async () => {
    if (!config) return;
    try {
      await updateSecurityConfig(config);
    } catch (e) {
      console.error('Failed to save security config', e);
    }
  };

  // Expose save to parent
  useEffect(() => {
    if (onSaveRef) onSaveRef.current = handleSave;
    return () => { if (onSaveRef) onSaveRef.current = null; };
  });

  if (loading) return <div className="p-10 text-center text-xs text-gray-400">{t('common.loading', 'Loading...')}</div>;
  if (!config) return <div className="p-10 text-center text-xs text-red-400">{t('security.config.load_failed', 'Failed to load config')}</div>;

  return (
    <div className="p-4 h-full overflow-y-auto">
      <div className="grid grid-cols-2 gap-4 h-full">
        {/* Left: Security Mode + spacer */}
        <div className="flex flex-col gap-4">
        <div className="bg-white dark:bg-base-200 rounded-lg border border-gray-100 dark:border-base-300 p-4">
          <div className="flex items-center gap-1.5 mb-1">
            <Shield size={14} className="text-blue-500" />
            <span className="text-xs font-semibold text-base-content">{t('security.config.mode_title', 'Security Mode')}</span>
          </div>
          <p className="text-[10px] text-gray-500 mb-3">{t('security.config.mode_desc', 'Controls how the gateway checks incoming IP addresses.')}</p>

          <div className="space-y-2 flex-1">
            {(['off', 'whitelist', 'blacklist'] as const).map(mode => (
              <label key={mode} className="flex items-start gap-3 p-2.5 rounded-lg hover:bg-gray-50 dark:hover:bg-base-300 transition-colors cursor-pointer">
                <input
                  type="radio"
                  name="security-mode"
                  className="mt-0.5 w-4 h-4 accent-blue-500"
                  checked={config.mode === mode}
                  onChange={() => setConfig({ ...config, mode })}
                />
                <div>
                  <span className="text-xs font-medium text-base-content">
                    {mode === 'off' ? t('security.config.mode_off_label', 'Off') : mode === 'whitelist' ? t('security.config.mode_whitelist_label', 'Whitelist') : t('security.config.mode_blacklist_label', 'Blacklist')}
                  </span>
                  <p className="text-[10px] text-gray-500 mt-0.5">
                    {mode === 'off' && t('security.config.mode_off', 'No IP checking — only API Key authentication')}
                    {mode === 'whitelist' && t('security.config.mode_whitelist', 'Only allow IPs in the whitelist (recommended for LAN sharing)')}
                    {mode === 'blacklist' && t('security.config.mode_blacklist', 'Block IPs in the blacklist, allow all others')}
                  </p>
                </div>
              </label>
            ))}
          </div>
        </div>
        <div className="flex-1" />
        </div>

        {/* Right: Auto Ban + Log Retention (internal builds only — no runtime enforcement in public) */}
        {showEnforcementControls && (
        <div className="flex flex-col gap-4">
          {/* Auto Ban */}
          <div className="bg-white dark:bg-base-200 rounded-lg border border-gray-100 dark:border-base-300 p-4">
            <div className="flex items-center gap-1.5 mb-1">
              <Shield size={14} className="text-red-500" />
              <span className="text-xs font-semibold text-base-content">{t('security.config.auto_ban_title', 'Auto Ban')}</span>
            </div>
            <p className="text-[10px] text-gray-500 mb-3">{t('security.config.auto_ban_desc', 'Automatically blacklist IPs after repeated auth failures.')}</p>

            <label className="flex items-center gap-3 cursor-pointer">
              <div className="relative inline-flex items-center">
                <input
                  type="checkbox"
                  className="sr-only peer"
                  checked={config.auto_ban_enabled}
                  onChange={(e) => setConfig({ ...config, auto_ban_enabled: e.target.checked })}
                />
                <div className="w-9 h-5 bg-gray-200 dark:bg-base-300 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-red-500" />
              </div>
              <span className="text-xs font-medium text-base-content">{t('security.config.enable_auto_ban', 'Enable auto-ban')}</span>
            </label>

            {config.auto_ban_enabled && (
              <div className="grid grid-cols-2 gap-3 mt-3 pt-3 border-t border-gray-100 dark:border-base-300">
                <div>
                  <label className="block text-[10px] text-gray-500 mb-1">{t('security.config.failure_threshold', 'Failure threshold')}</label>
                  <input
                    type="number"
                    className="w-full px-2.5 py-1.5 text-xs border border-gray-200 dark:border-gray-600 rounded-lg bg-white dark:bg-base-100 text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
                    value={config.auto_ban_threshold}
                    onChange={e => setConfig({ ...config, auto_ban_threshold: parseInt(e.target.value) || 10 })}
                  />
                </div>
                <div>
                  <label className="block text-[10px] text-gray-500 mb-1">{t('security.config.ban_duration', 'Ban duration (seconds)')}</label>
                  <input
                    type="number"
                    className="w-full px-2.5 py-1.5 text-xs border border-gray-200 dark:border-gray-600 rounded-lg bg-white dark:bg-base-100 text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
                    value={config.auto_ban_duration_secs}
                    onChange={e => setConfig({ ...config, auto_ban_duration_secs: parseInt(e.target.value) || 3600 })}
                  />
                </div>
              </div>
            )}
          </div>

          {/* Log Retention */}
          <div className="bg-white dark:bg-base-200 rounded-lg border border-gray-100 dark:border-base-300 p-4">
            <div className="flex items-center gap-1.5 mb-1">
              <ShieldCheck size={14} className="text-green-500" />
              <span className="text-xs font-semibold text-base-content">{t('security.config.log_retention_title', 'Log Retention')}</span>
            </div>
            <div className="flex items-center gap-3 mt-2">
              <label className="text-[10px] text-gray-500">{t('security.config.keep_logs_days', 'Keep logs for (days)')}</label>
              <input
                type="number"
                className="w-20 px-2.5 py-1.5 text-xs border border-gray-200 dark:border-gray-600 rounded-lg bg-white dark:bg-base-100 text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
                value={config.log_retention_days}
                onChange={e => setConfig({ ...config, log_retention_days: parseInt(e.target.value) || 30 })}
              />
            </div>
          </div>
        </div>
        )}
      </div>
    </div>
  );
};
