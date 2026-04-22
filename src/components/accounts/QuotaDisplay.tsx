/**
 * Quota display components:
 * - QuotaItem: capsule tags for account table (AM QuotaItem style)
 * - QuotaDetail: progress bars for account details dialog (CC CLI style)
 *
 * Data source: /api/oauth/usage (Utilization) — live query, no quota consumed.
 */
import { useState, useEffect, useCallback } from 'react';
import { RefreshCw } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { cn } from '../../utils/cn';
import type { RateLimit, Utilization } from '../../types/account';

// ─── Helpers ────────────────────────────────────────────────

function formatCountdown(isoDate: string): string {
  const resetMs = new Date(isoDate).getTime();
  const diff = Math.max(0, Math.floor((resetMs - Date.now()) / 1000));
  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  const mins = Math.floor((diff % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}


// ─── Hook: fetch usage from Tauri command ───────────────────

export function useAccountUsage(accountId: string | null, autoFetch = false) {
  const [usage, setUsage] = useState<Utilization | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!accountId) return;
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<Utilization>('get_account_quota', { accountId });
      setUsage(data);
    } catch (e) {
      setError(String(e));
    }
    setLoading(false);
  }, [accountId]);

  useEffect(() => {
    if (accountId && autoFetch) refresh();
  }, [accountId, autoFetch, refresh]);

  return { usage, loading, error, refresh };
}

// ─── QuotaItem (for AccountTable — AM capsule style) ──

interface QuotaItemProps {
  usage?: Utilization | null;
}

export function QuotaItem({ usage }: QuotaItemProps) {
  if (!usage) {
    return <span className="text-xs text-gray-500">&mdash;</span>;
  }

  return (
    <div className="grid grid-cols-2 gap-x-4">
      <QuotaMiniBar label="Session (5h)" window={usage.five_hour} defaultReset="5h" />
      <QuotaMiniBar label="Weekly (7d)" window={usage.seven_day} defaultReset="7d" />
    </div>
  );
}

function QuotaMiniBar({ label, window: w, defaultReset }: { label: string; window: RateLimit | null; defaultReset?: string }) {
  if (!w || w.utilization == null) {
    return (
      <div>
        <div className="flex items-center justify-between">
          <span className="text-xs text-gray-500">{label}</span>
          <span className="text-xs text-gray-600">&mdash;</span>
        </div>
        <div className="w-full bg-gray-700 rounded-full h-1.5 mt-1">
          <div className="h-1.5 rounded-full bg-gray-600" style={{ width: '0%' }} />
        </div>
      </div>
    );
  }

  const used = Math.round(w.utilization);
  return (
    <div>
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-gray-300">{label}</span>
        <span className="text-xs font-bold text-gray-200">{used}%</span>
      </div>
      <div className="w-full bg-gray-700 rounded-full h-1.5 mt-1 overflow-hidden">
        <div className="h-1.5 rounded-full bg-blue-500 transition-all" style={{ width: `${Math.min(used, 100)}%` }} />
      </div>
      <div className="text-[10px] text-gray-500 mt-0.5 h-[14px]">
        {w.resets_at ? `Resets in ${formatCountdown(w.resets_at)}` : defaultReset ? `Resets in ${defaultReset}` : '\u00a0'}
      </div>
    </div>
  );
}

// ─── QuotaDetail (for AccountDetailsDialog — CC CLI style) ──

interface QuotaDetailProps {
  accountId: string;
  planTier?: string;
  cached?: Utilization;
}

export function QuotaDetail({ accountId, planTier, cached }: QuotaDetailProps) {
  const { usage: liveUsage, loading, error, refresh } = useAccountUsage(accountId, false);
  const usage = liveUsage || cached || null; // Prefer live data, fallback to cache

  if (error) {
    return (
      <div className="text-xs text-red-400 py-2">
        {error}
        <button onClick={refresh} className="ml-2 text-blue-400 hover:text-blue-300">retry</button>
      </div>
    );
  }

  if (!usage) {
    return (
      <div className="flex items-center gap-2 text-xs text-gray-500 py-2">
        {loading ? 'Loading...' : 'No usage data'}
        {!loading && (
          <button onClick={refresh} className="text-blue-400 hover:text-blue-300">
            <RefreshCw className="w-3 h-3" />
          </button>
        )}
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-xs font-semibold text-gray-300 uppercase tracking-wider">Usage</span>
        <div className="flex items-center gap-2">
          {planTier && <span className="text-[10px] font-medium text-gray-500">{planTier}</span>}
          <button
            onClick={refresh}
            disabled={loading}
            className="text-gray-500 hover:text-gray-300 transition-colors"
          >
            <RefreshCw className={cn("w-3 h-3", loading && "animate-spin")} />
          </button>
        </div>
      </div>

      <QuotaBar label="Session (5h)" window={usage.five_hour} />
      <QuotaBar label="Weekly (7 day)" window={usage.seven_day} />
      <QuotaBar label="Weekly Sonnet" window={usage.seven_day_sonnet} />
    </div>
  );
}

function QuotaBar({ label, window: w }: {
  label: string;
  window: RateLimit | null;
}) {
  if (!w || w.utilization == null) {
    return (
      <div>
        <div className="flex items-center justify-between mb-1">
          <span className="text-xs font-medium text-gray-400">{label}</span>
          <span className="text-xs text-gray-600">&mdash;</span>
        </div>
        <div className="w-full bg-gray-700 rounded-full h-1.5">
          <div className="h-1.5 rounded-full bg-gray-600" style={{ width: '0%' }} />
        </div>
      </div>
    );
  }

  const used = Math.round(w.utilization);
  return (
    <div>
      <div className="flex items-center justify-between mb-1">
        <span className="text-xs font-medium text-gray-300">{label}</span>
        <span className="text-xs font-semibold text-gray-300">
          {used}%
        </span>
      </div>
      <div className="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
        <div
          className="h-1.5 rounded-full transition-all bg-blue-500"
          style={{ width: `${Math.min(used, 100)}%` }}
        />
      </div>
      {w.resets_at && (
        <div className="text-[10px] text-gray-500 mt-0.5">
          Resets in {formatCountdown(w.resets_at)}
        </div>
      )}
    </div>
  );
}
