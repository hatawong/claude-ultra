// ─── Quota (from /api/oauth/usage) ──────────────────────────

export interface RateLimit {
  utilization: number | null;  // 0-100 percentage
  resets_at: string | null;    // ISO 8601
}

export interface Utilization {
  five_hour: RateLimit | null;
  seven_day: RateLimit | null;
  seven_day_sonnet: RateLimit | null;
  seven_day_opus: RateLimit | null;
  extra_usage: {
    is_enabled: boolean;
    monthly_limit: number | null;
    used_credits: number | null;
    utilization: number | null;
  } | null;
}

// ─── Sub-structures ──────────────────────────────────────────

export interface AndroidDevice {
  deviceId: string;
  androidId: string;
  platform: string;
  manufacturer: string;
  model: string;
  osVersion: string;
  releaseVersion: string;
  locale: string;
  timezone: string;
  buildTags: string;
  carrierName: string;
  carrierCountry: string;
  appVersion: string;
  anthropicDeviceId: string;
}

export interface AndroidClient {
  device: AndroidDevice;
  sessionKey: string;
  routingHint: string;
  lastActivity: number | null;
}

export interface CliClient {
  accessToken: string;
  refreshToken: string;
  expiresAt: number;
  scopes?: string[];
  lastActivity: number | null;
}

export interface WebClient {
  cookies: unknown[];
  localStorage: Record<string, string>;
  lastActivity: number | null;
}

export type StaticProtocol = 'socks5' | 'http';

export interface StaticProxy {
  protocol: StaticProtocol;
  host: string;
  port: number;
  username: string;
  password: string;
}

export interface ProxySection {
  sessionId: string;
  host: string;
  lastIp: string | null;
  country: string | null;
  region: string | null;
  city: string | null;
  isp: string | null;
  quality: string | null;
  lastChecked: number | null;
  type?: string;       // residential | mobile
  lifetime?: string;   // 24h etc.
  createdAt?: number;  // session creation time (ms)
}

/**
 * Result returned by test_proxy_connection / test_static_proxy_dryrun /
 * test_static_proxy_connection IPC. Mirrors Rust commands::proxy::ProxyTestResult.
 *
 * proxySection is populated only by static-proxy probe paths so the Save flow
 * can forward it to update_account_route.probedProxy / add_account_and_login
 * for atomic write of route_mode + staticProxy + proxy snapshot.
 */
export interface ProxyTestResult {
  ok: boolean;
  mode: string;
  ip: string | null;
  country: string | null;
  error: string | null;
  proxySection?: ProxySection | null;
}

// ─── Account V3 ──────────────────────────────────────────────

export interface Account {
  // Core identity
  accountId: string;
  email: string;
  phoneNumber: string;
  fullName: string;
  customLabel: string | null;
  accountUuid: string;
  orgId: string;
  region: string;
  country?: string;
  createdAt: number;

  // Routing (per-account)
  routeMode: string;        // "proxy" | "static" | "vercel" | "direct"
  routeCountry?: string;    // proxy target country, null = follow registration country
  staticProxy?: StaticProxy; // per-account static residential proxy (used when routeMode === "static")

  // Plan
  subscriptionType: string;
  rateLimitTier: string | null;
  subscriptionRenewAt: number | null;
  subscriptionCreatedAt: number | null;
  billingType: string | null;
  hasExtraUsageEnabled: boolean;

  // Login method
  loginMethod: string;

  // Status
  disabled: boolean;
  disabledReason: string | null;
  disabledAt: number | null;
  userDisabled: boolean;
  userDisabledReason: string | null;
  userDisabledAt: number | null;

  // Proxy
  proxy: ProxySection | null;

  // Utilization (from /api/oauth/usage)
  utilization: Utilization | null;

  // Three clients
  android: AndroidClient | null;
  web: WebClient | null;
  cli: CliClient | null;

  // V1 compat
  plan?: string;
}

export type FilterType = 'all' | 'pro' | 'max' | 'free';
export type ViewMode = 'list' | 'grid';

export function getLifecycleStage(account: Account): string {
  if (account.cli) return 'CLI 已授权';
  if (account.subscriptionType && account.subscriptionType !== 'free') return '已支付';
  if (account.web) return '已登录';
  if (account.android?.sessionKey) return '已注册';
  return '未知';
}

export function getLifecycleColor(stage: string): string {
  switch (stage) {
    case 'CLI 已授权': return 'green';
    case '已支付': return 'green';
    case '已登录': return 'blue';
    case '已注册': return 'orange';
    default: return 'default';
  }
}

export function getPlanColor(plan: string): string {
  switch (plan) {
    case 'max': return 'purple';
    case 'pro': return 'blue';
    default: return 'default';
  }
}

/** Get device model from android sub-object */
export function getDeviceModel(account: Account): string {
  return account.android?.device?.model || 'Unknown';
}

/** Get Plan label text */
export function getPlanLabel(account: Account): string {
  const type = (account.subscriptionType || account.plan || 'free').toLowerCase();
  if (type === 'free') return 'Free';
  let label = type === 'max' ? 'Max' : type === 'pro' ? 'Pro' : type;
  // Extract multiplier from rateLimitTier (e.g. default_claude_max_5x → 5x)
  const tier = account.rateLimitTier || '';
  const multiplierMatch = tier.match(/(\d+x)$/);
  if (multiplierMatch) {
    label += ` ${multiplierMatch[1]}`;
  }
  if (account.subscriptionRenewAt && account.subscriptionRenewAt < Date.now()) {
    return `${label} \u00b7 \u5df2\u8fc7\u671f`;
  }
  return label;
}

/** Get Plan badge color class */
export function getPlanBadgeClass(account: Account): string {
  const type = (account.subscriptionType || account.plan || 'free').toLowerCase();
  if (type === 'free') return 'bg-gray-100 dark:bg-gray-500/20 text-gray-600 dark:text-gray-400';
  if (account.subscriptionRenewAt && account.subscriptionRenewAt < Date.now()) {
    return 'bg-gray-100 dark:bg-gray-500/20 text-gray-600 dark:text-gray-400';
  }
  return 'bg-green-100 dark:bg-green-500/20 text-green-700 dark:text-green-400';
}

/** Get anomaly label (mutually exclusive, priority order) */
export function getAnomalyLabel(account: Account): { text: string; color: string } | null {
  if (account.disabled) {
    const reason = (account.disabledReason || '').toLowerCase();
    if (reason.includes('banned') || reason.includes('forbidden')) {
      return { text: '\u5df2\u5c01\u7981', color: 'bg-red-100 dark:bg-red-500/20 text-red-700 dark:text-red-400' };
    }
    if (reason.includes('invalid_grant') || reason.includes('expired')) {
      return { text: '\u5df2\u5931\u6548', color: 'bg-orange-100 dark:bg-orange-500/20 text-orange-700 dark:text-orange-400' };
    }
    return { text: '\u5df2\u7981\u7528', color: 'bg-red-100 dark:bg-red-500/20 text-red-600 dark:text-red-400' };
  }
  if (account.userDisabled) {
    return { text: '\u5df2\u505c\u7528', color: 'bg-gray-100 dark:bg-gray-500/20 text-gray-600 dark:text-gray-400' };
  }
  return null;
}

/** Get last activity timestamp (max of all clients) */
export function getLastActivity(account: Account): number | null {
  const times = [
    account.android?.lastActivity,
    account.web?.lastActivity,
    account.cli?.lastActivity,
  ].filter((t): t is number => t != null);
  return times.length > 0 ? Math.max(...times) : null;
}

/** Format timestamp to YYYY/M/D HH:mm */
export function formatActivityTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** Truncate sensitive string for display */
export function truncateSensitive(value: string, length = 20): string {
  if (value.length <= length) return value;
  return value.slice(0, length) + '...';
}
