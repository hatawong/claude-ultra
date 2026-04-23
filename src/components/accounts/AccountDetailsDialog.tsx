import { useState, useEffect } from 'react';
import { X, Copy, ChevronDown, ChevronUp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import type { Account } from '../../types/account';
import {
  getPlanLabel, getPlanBadgeClass, getAnomalyLabel,
  formatActivityTime, truncateSensitive,
} from '../../types/account';
import { cn } from '../../utils/cn';
import { QuotaDetail } from './QuotaDisplay';
import { useConfigStore } from '../../stores/useConfigStore';

interface AccountDetailsDialogProps {
  account: Account;
  onClose: () => void;
  onDelete: (account: Account) => void;
  onToggleProxy: (account: Account) => void;
  onToast: (msg: string) => void;
  onUpdated?: () => void;
}

type TabId = 'overview' | 'proxy' | 'credentials' | 'anomaly';

function AccountDetailsDialog({
  account, onClose, onDelete, onToggleProxy, onToast, onUpdated,
}: AccountDetailsDialogProps) {
  const { t } = useTranslation();
  const hasAnomaly = account.disabled || account.userDisabled;

  const tabs: { id: TabId; label: string; show: boolean }[] = [
    { id: 'overview', label: t('accounts.details.overview', 'Overview'), show: true },
    { id: 'proxy', label: t('accounts.details.proxy_tab', 'Proxy'), show: true },
    { id: 'credentials', label: t('accounts.details.credentials', 'Credentials'), show: true },
    { id: 'anomaly', label: `\u26a0\ufe0f ${t('accounts.details.anomaly', 'Anomaly')}`, show: hasAnomaly },
  ];

  const [activeTab, setActiveTab] = useState<TabId>('overview');

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={onClose}>
      <div
        className="bg-white dark:bg-base-100 rounded-2xl shadow-2xl w-[600px] max-h-[80vh] flex flex-col border border-gray-200 dark:border-base-300"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 pt-5 pb-3">
          <div>
            <div className="text-sm font-semibold text-gray-900 dark:text-base-content">
              {account.email}
            </div>
            <div className="flex items-center gap-1 mt-1">
              <span className={cn('px-1.5 py-0 rounded text-[10px] font-semibold', getPlanBadgeClass(account))}>
                {getPlanLabel(account)}
              </span>
              {account.customLabel && (
                <span className="px-1.5 py-0 rounded text-[10px] font-semibold bg-orange-100 dark:bg-orange-500/20 text-orange-700 dark:text-orange-400">
                  {account.customLabel}
                </span>
              )}
            </div>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-md text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 hover:bg-gray-100 dark:hover:bg-base-200 transition-colors">
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Tabs */}
        <div className="px-6 flex gap-1 border-b border-gray-100 dark:border-base-200">
          {tabs.filter((t) => t.show).map((tab) => (
            <button
              key={tab.id}
              className={cn(
                'px-3 py-2 text-xs font-medium border-b-2 transition-colors',
                activeTab === tab.id
                  ? 'border-blue-500 text-blue-600 dark:text-blue-400'
                  : 'border-transparent text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300',
              )}
              onClick={() => setActiveTab(tab.id)}
            >
              {tab.label}
            </button>
          ))}
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-6 py-4 text-sm">
          {activeTab === 'overview' && <OverviewTab account={account} onToast={onToast} onUpdated={onUpdated} />}
          {activeTab === 'proxy' && <ProxyTab account={account} onToast={onToast} />}
          {activeTab === 'credentials' && <CredentialsTab account={account} onToast={onToast} />}
          {activeTab === 'anomaly' && <AnomalyTab account={account} onDelete={onDelete} onToggleProxy={onToggleProxy} onToast={onToast} />}
        </div>

        {/* Footer */}
        <div className="px-6 py-3 border-t border-gray-100 dark:border-base-200 flex justify-end">
          <button
            onClick={onClose}
            className="px-4 py-1.5 text-xs font-medium bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 rounded-lg hover:bg-gray-200 dark:hover:bg-base-300 transition-colors"
          >
            {t('common.close', 'Close')}
          </button>
        </div>
      </div>
    </div>
  );
}

// ─── Overview Tab ───────────────────────────────────────────

function OverviewTab({ account, onToast, onUpdated }: { account: Account; onToast: (msg: string) => void; onUpdated?: () => void }) {
  const { t } = useTranslation();
  const planLabel = getPlanLabel(account);

  return (
    <div className="space-y-4">
      <SectionTitle text={t('accounts.details.basic_info', 'Basic Info')} />
      <InfoGrid>
        <InfoRow label={t('accounts.details.account_id', 'Account ID')} value={account.accountId} />
        <InfoRow label={t('accounts.details.created_at', 'Created')} value={account.createdAt ? formatActivityTime(account.createdAt) : '\u2014'} />
        <InfoRow label="Plan" value={planLabel + (account.subscriptionRenewAt ? ` \u00b7 ${t('accounts.details.renews', 'Renews')} ${formatActivityTime(account.subscriptionRenewAt)}` : '')} />
        <InfoRow label={t('accounts.details.billing', 'Billing')} value={account.billingType || '\u2014'} />
        <InfoRow label={t('accounts.details.rate_limit', 'Rate Limit Tier')} value={account.rateLimitTier || '\u2014'} />
        <InfoRow label={t('accounts.details.country', 'Country')} value={(account.country || account.region || '—').toUpperCase()} />
        <span className="text-xs text-gray-500 whitespace-nowrap">{t('accounts.table.route', 'Route')}</span>
        <RouteEditor account={account} onToast={onToast} onUpdated={onUpdated} />
      </InfoGrid>

      <SectionTitle text={t('accounts.details.quota_section', 'Quota')} />
      <QuotaDetail accountId={account.accountId} planTier={account.rateLimitTier || undefined} cached={account.utilization || undefined} />

      <SectionTitle text={t('accounts.details.status', 'Status')} />
      <InfoGrid>
        <InfoRow label={t('accounts.details.enabled', 'Enabled')} value={account.disabled ? '\u274c ' + t('common.disabled', 'Disabled') : '\u2705 ' + t('common.enabled', 'Enabled')} />
        <InfoRow label={t('accounts.details.system_disabled', 'System Disabled')} value={account.disabled ? (account.disabledReason || 'Yes') : t('accounts.details.no', 'No')} />
        <InfoRow label={t('accounts.details.extra_usage', 'Extra Usage')} value={account.hasExtraUsageEnabled ? t('common.enabled', 'Enabled') : t('accounts.details.not_enabled', 'Not Enabled')} />
      </InfoGrid>
    </div>
  );
}

// ─── Proxy Tab ──────────────────────────────────────────────

function ProxyTab({ account }: { account: Account; onToast: (msg: string) => void }) {
  const { t } = useTranslation();
  const proxy = account.proxy;

  if (!proxy) {
    return (
      <div className="py-4 text-xs text-gray-500 dark:text-gray-400">
        {t('accounts.details.no_proxy', 'No proxy assigned')}
      </div>
    );
  }

  return (
    <div className="space-y-2">
      <InfoGrid>
        <InfoRow label="IP" value={proxy.lastIp || '\u2014'} />
        <InfoRow label={t('accounts.details.proxy_country', 'Country')} value={proxy.country || '\u2014'} />
        <InfoRow label="ISP" value={proxy.isp || '\u2014'} />
        {(proxy.region || proxy.city) && (
          <InfoRow label={t('accounts.details.proxy_region', 'Region')} value={[proxy.region, proxy.city].filter(Boolean).join(' \u00b7 ')} />
        )}
        <InfoRow label="Session" value={proxy.sessionId} />
        <InfoRow label={t('accounts.details.quality', 'Quality')} value={proxy.quality || '\u2014'} />
        <InfoRow label={t('accounts.details.last_checked', 'Last Checked')} value={proxy.lastChecked ? formatActivityTime(proxy.lastChecked) : '\u2014'} />
      </InfoGrid>
    </div>
  );
}

// ─── Credentials Tab ────────────────────────────────────────

function CredentialsTab({ account, onToast }: { account: Account; onToast: (msg: string) => void }) {
  const { t } = useTranslation();
  const notImplemented = () => onToast(t('common.coming_soon', 'Coming soon'));

  return (
    <div className="space-y-5">
      {/* Android */}
      <div>
        <SectionTitle text={t('accounts.details.android', 'Android Client')} />
        {account.android ? (
          <InfoGrid>
            <InfoRowCopyable
              label="sessionKey"
              value={account.android.sessionKey}
              onCopy={() => { navigator.clipboard.writeText(account.android!.sessionKey); onToast(t('common.copied', 'Copied')); }}
            />
            <InfoRow label={t('accounts.details.device', 'Device')} value={`${account.android.device.model} \u00b7 Android ${account.android.device.releaseVersion}`} />
            <InfoRow label={t('accounts.details.carrier', 'Carrier')} value={`${account.android.device.carrierName} \u00b7 ${account.android.device.carrierCountry.toUpperCase()}`} />
            <InfoRow label={t('accounts.details.last_activity', 'Last Activity')} value={account.android.lastActivity ? formatActivityTime(account.android.lastActivity) : '\u2014'} />
            <div className="col-span-2 pt-1">
              <button onClick={notImplemented} className="px-3 py-1 text-xs font-medium bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors">
                {account.android.lastActivity ? t('accounts.details.keepalive', 'Keepalive') : t('accounts.details.login', 'Login')}
              </button>
            </div>
          </InfoGrid>
        ) : (
          <div className="text-xs text-gray-400 dark:text-gray-500 py-2">{t('accounts.details.not_configured', 'Not configured')}</div>
        )}
      </div>

      {/* Web */}
      <div>
        <SectionTitle text={t('accounts.details.web', 'Web Client')} />
        {account.web ? (
          <InfoGrid>
            <InfoRow label="Cookies" value={`${account.web.cookies?.length || 0} ${t('accounts.details.cookies_count', 'cookies')}`} />
            <InfoRow label={t('accounts.details.last_activity', 'Last Activity')} value={account.web.lastActivity ? formatActivityTime(account.web.lastActivity) : '\u2014'} />
            <div className="col-span-2 pt-1">
              <button onClick={notImplemented} className="px-3 py-1 text-xs font-medium bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors">
                {t('accounts.details.keepalive', 'Keepalive')}
              </button>
            </div>
          </InfoGrid>
        ) : (
          <div className="text-xs text-gray-400 dark:text-gray-500 py-2">
            {t('accounts.details.not_configured', 'Not configured')}
            <button onClick={notImplemented} className="ml-3 px-3 py-1 text-xs font-medium bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors">
              {t('accounts.details.login', 'Login')}
            </button>
          </div>
        )}
      </div>

      {/* CLI */}
      <div>
        <SectionTitle text={t('accounts.details.cli', 'CLI Client')} />
        {account.cli ? (
          <InfoGrid>
            <InfoRowCopyable
              label="accessToken"
              value={account.cli.accessToken}
              onCopy={() => { navigator.clipboard.writeText(account.cli!.accessToken); onToast(t('common.copied', 'Copied')); }}
            />
            <InfoRowCopyable
              label="refreshToken"
              value={account.cli.refreshToken}
              onCopy={() => { navigator.clipboard.writeText(account.cli!.refreshToken); onToast(t('common.copied', 'Copied')); }}
            />
            <InfoRow
              label={t('accounts.details.expires', 'Expires')}
              value={account.cli.expiresAt ? formatActivityTime(account.cli.expiresAt) + (account.cli.expiresAt < Date.now() ? ` (\u5df2\u8fc7\u671f)` : '') : '\u2014'}
            />
            {account.cli.scopes && account.cli.scopes.length > 0 && (
              <InfoRow label="Scopes" value={account.cli.scopes.join(', ')} />
            )}
            <InfoRow label={t('accounts.details.last_activity', 'Last Activity')} value={account.cli.lastActivity ? formatActivityTime(account.cli.lastActivity) : '\u2014'} />
          </InfoGrid>
        ) : (
          <div className="text-xs text-gray-400 dark:text-gray-500 py-2">
            {t('accounts.details.not_configured', 'Not configured')}
            <button onClick={notImplemented} className="ml-3 px-3 py-1 text-xs font-medium bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors">
              OAuth
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

// ─── Anomaly Tab ────────────────────────────────────────────

function AnomalyTab({ account, onDelete, onToggleProxy, onToast }: {
  account: Account;
  onDelete: (account: Account) => void;
  onToggleProxy: (account: Account) => void;
  onToast: (msg: string) => void;
}) {
  const { t } = useTranslation();
  const [rawExpanded, setRawExpanded] = useState(false);
  const anomaly = getAnomalyLabel(account);
  const notImplemented = () => onToast(t('common.coming_soon', 'Coming soon'));

  const rawReason = account.disabledReason || account.userDisabledReason || '';
  const displayReason = (() => {
    const match = rawReason.match(/^(HTTP \d+): (.+)$/s);
    if (!match) return rawReason;
    const [, prefix, body] = match;
    try {
      const json = JSON.parse(body);
      const msg = json?.error?.message;
      if (msg) return `${prefix}: ${msg}`;
    } catch {}
    return rawReason;
  })();
  const isBanned = rawReason.toLowerCase().includes('banned') || rawReason.toLowerCase().includes('forbidden') || rawReason.toLowerCase().includes('permission_error');
  const isExpired = rawReason.toLowerCase().includes('invalid_grant') || rawReason.toLowerCase().includes('expired');

  return (
    <div className="space-y-4">
      <SectionTitle text={t('accounts.details.anomaly_info', 'Anomaly Info')} />
      <InfoGrid>
        <InfoRow label={t('accounts.details.anomaly_type', 'Type')} value={anomaly?.text || '\u2014'} />
        <InfoRow label={t('accounts.details.anomaly_time', 'Time')} value={(account.disabledAt || account.userDisabledAt) ? formatActivityTime(account.disabledAt || account.userDisabledAt!) : '\u2014'} />
        <InfoRow label={t('common.reason', 'Reason')} value={displayReason || '\u2014'} />
      </InfoGrid>

      {rawReason && (
        <>
          <div className="flex items-center justify-between">
            <SectionTitle text={t('accounts.details.raw_response', 'Raw Response')} />
            <button
              onClick={() => setRawExpanded(!rawExpanded)}
              className="flex items-center gap-1 text-xs text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
            >
              {rawExpanded ? <ChevronUp className="w-3 h-3" /> : <ChevronDown className="w-3 h-3" />}
              {rawExpanded ? t('common.collapse', 'Collapse') : t('common.expand', 'Expand')}
            </button>
          </div>
          {rawExpanded && (
            <pre className="text-[10px] bg-gray-50 dark:bg-base-200 p-3 rounded-lg overflow-x-auto text-gray-600 dark:text-gray-400 font-mono whitespace-pre-wrap break-all">
              {(() => {
                const match = rawReason.match(/^(HTTP \d+): (.+)$/s);
                if (!match) return rawReason;
                const [, prefix, body] = match;
                try { return `${prefix}:\n${JSON.stringify(JSON.parse(body), null, 2)}`; } catch {}
                return rawReason;
              })()}
            </pre>
          )}
        </>
      )}

      <SectionTitle text={t('accounts.details.suggested_action', 'Suggested Action')} />
      <div className="space-y-2">
        {isBanned && (
          <>
            <p className="text-xs text-gray-500 dark:text-gray-400">
              {t('accounts.details.banned_suggestion', 'Account permanently banned by Anthropic. Cannot be recovered. Suggest deleting.')}
            </p>
            <button
              onClick={() => onDelete(account)}
              className="px-3 py-1.5 text-xs font-medium bg-red-500 text-white rounded-md hover:bg-red-600 transition-colors"
            >
              {t('accounts.details.delete_account', 'Delete Account')}
            </button>
          </>
        )}
        {isExpired && (
          <>
            <p className="text-xs text-gray-500 dark:text-gray-400">
              {t('accounts.details.expired_suggestion', 'Token expired or OAuth invalidated. Re-authorize to recover.')}
            </p>
            <button
              onClick={notImplemented}
              className="px-3 py-1.5 text-xs font-medium bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors"
            >
              {t('accounts.details.oauth_reauth', 'OAuth Re-authorize')}
            </button>
          </>
        )}
        {!isBanned && !isExpired && account.userDisabled && (
          <>
            <p className="text-xs text-gray-500 dark:text-gray-400">
              {t('accounts.details.stopped_suggestion', 'Manually disabled. Can be re-enabled at any time.')}
            </p>
            <button
              onClick={() => onToggleProxy(account)}
              className="px-3 py-1.5 text-xs font-medium bg-green-500 text-white rounded-md hover:bg-green-600 transition-colors"
            >
              {t('accounts.enable_user', 'Enable')}
            </button>
          </>
        )}
      </div>
    </div>
  );
}

// ─── Shared UI Primitives ───────────────────────────────────

function SectionTitle({ text }: { text: string }) {
  return (
    <div className="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider border-b border-gray-100 dark:border-base-200 pb-1">
      {text}
    </div>
  );
}

function InfoGrid({ children }: { children: React.ReactNode }) {
  return <div className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">{children}</div>;
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <>
      <span className="text-xs text-gray-400 dark:text-gray-500 whitespace-nowrap">{label}</span>
      <span className="text-xs text-gray-700 dark:text-gray-300 break-all">{value}</span>
    </>
  );
}

function InfoRowCopyable({ label, value, onCopy }: { label: string; value: string; onCopy: () => void }) {
  return (
    <>
      <span className="text-xs text-gray-400 dark:text-gray-500 whitespace-nowrap">{label}</span>
      <span className="text-xs text-gray-700 dark:text-gray-300 flex items-center gap-1.5">
        <span>{truncateSensitive(value)}</span>
        <button
          onClick={onCopy}
          className="p-0.5 text-gray-400 hover:text-blue-500 transition-colors"
          title="Copy"
        >
          <Copy className="w-3 h-3" />
        </button>
      </span>
    </>
  );
}

const PROXY_COUNTRIES = ['us', 'jp', 'kr', 'ph'];

function RouteEditor({ account, onToast, onUpdated }: { account: Account; onToast: (msg: string) => void; onUpdated?: () => void }) {
  const { t } = useTranslation();
  const config = useConfigStore((s) => s.config);
  const proxyAvailable = !!config?.proxy?.residential?.username && !!config?.proxy?.residential?.password;
  const vercelAvailable = !!config?.gateway?.vercel_api_key;

  const currentMode = (account.routeMode || 'proxy').toLowerCase();
  const currentCountry = (account.routeCountry || account.country || account.region || 'us').toLowerCase();
  const [mode, setMode] = useState(currentMode);
  const [country, setCountry] = useState(currentCountry);
  const [saving, setSaving] = useState(false);

  useEffect(() => { setMode(currentMode); setCountry(currentCountry); }, [currentMode, currentCountry]);

  const handleSave = async (newMode: string, newCountry?: string) => {
    setMode(newMode);
    if (newCountry !== undefined) setCountry(newCountry);
    setSaving(true);
    try {
      await invoke('update_account_route', {
        accountId: account.accountId,
        routeMode: newMode,
        ...(newMode === 'proxy' && newCountry !== undefined ? { routeCountry: newCountry } : {}),
      });
      onToast(t('accounts.details.route_updated', 'Route updated'));
      onUpdated?.();
    } catch (e) {
      onToast(String(e));
      setMode(currentMode);
      setCountry(currentCountry);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex items-center gap-1.5">
      <select
        value={mode}
        onChange={(e) => handleSave(e.target.value, country)}
        disabled={saving}
        className="text-xs bg-base-200 border border-base-300 rounded px-2 py-0.5 text-base-content focus:outline-none focus:ring-1 focus:ring-blue-500"
      >
        <option value="proxy" disabled={!proxyAvailable}>
          {t('accounts.route.proxy', 'Proxy')}
        </option>
        <option value="vercel" disabled={!vercelAvailable}>
          {t('accounts.route.vercel', 'Vercel')}
        </option>
        <option value="direct">
          {t('accounts.route.direct', 'Direct')}
        </option>
      </select>
      {mode === 'proxy' && (
        <select
          value={country}
          onChange={(e) => handleSave(mode, e.target.value)}
          disabled={saving}
          className="text-xs bg-base-200 border border-base-300 rounded px-1.5 py-0.5 text-base-content focus:outline-none focus:ring-1 focus:ring-blue-500 w-16"
        >
          {PROXY_COUNTRIES.map((c) => (
            <option key={c} value={c}>{c.toUpperCase()}</option>
          ))}
        </select>
      )}
    </div>
  );
}

export default AccountDetailsDialog;
