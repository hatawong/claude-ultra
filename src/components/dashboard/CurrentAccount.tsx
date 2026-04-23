import { Star } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Account } from '../../types/account';
import { getPlanLabel, getPlanBadgeClass } from '../../types/account';

interface RecommendedAccountProps {
  accounts: Account[];
}

function formatCountdown(isoDate: string): string {
  const diff = Math.max(0, Math.floor((new Date(isoDate).getTime() - Date.now()) / 1000));
  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  const mins = Math.floor((diff % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

/** Extract multiplier from rateLimitTier */
function getTierMultiplier(account: Account): number {
  const tier = account.rateLimitTier || '';
  const match = tier.match(/(\d+)x$/);
  if (match) return parseInt(match[1]);
  const type = (account.subscriptionType || 'free').toLowerCase();
  if (type === 'max') return 5;
  return 1;
}

/** Weighted remaining quota = (100 - utilization) * multiplier */
function weightedRemaining(account: Account): number {
  const util = account.utilization?.five_hour?.utilization;
  if (util == null) return -1;
  return (100 - util) * getTierMultiplier(account);
}

function QuotaBar({ label, utilization, resetsAt, defaultReset }: {
  label: string;
  utilization: number | null | undefined;
  resetsAt: string | null | undefined;
  defaultReset?: string;
}) {
  if (utilization == null) {
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

  const used = Math.round(utilization);
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
        {resetsAt ? `Resets in ${formatCountdown(resetsAt)}` : defaultReset ? `Resets in ${defaultReset}` : '\u00a0'}
      </div>
    </div>
  );
}

function RecommendedAccount({ accounts }: RecommendedAccountProps) {
  const { t } = useTranslation();

  // Pick the account with the highest weighted remaining quota
  const best = accounts
    .filter(a => weightedRemaining(a) >= 0)
    .sort((a, b) => weightedRemaining(b) - weightedRemaining(a))[0] || null;

  const planLabel = best ? getPlanLabel(best) : '';
  const planClass = best ? getPlanBadgeClass(best) : '';

  return (
    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 h-full flex flex-col">
      <h2 className="text-base font-semibold text-gray-900 dark:text-base-content mb-3 flex items-center gap-2">
        <Star className="w-4 h-4 text-amber-500" />
        {t('dashboard.recommended_account', 'Recommended Account')}
      </h2>

      {!best ? (
        <div className="text-center py-4 text-gray-400 dark:text-gray-500 text-sm flex-1 flex items-center justify-center">
          {t('dashboard.no_quota_data', 'No quota data. Refresh quota first.')}
        </div>
      ) : (
        <div className="flex-1">
          {/* Email + Plan badge */}
          <div className="flex items-center gap-2 mb-3">
            <span className={`inline-flex items-center px-1.5 py-0 rounded text-[10px] font-semibold shrink-0 ${planClass}`}>
              {planLabel}
            </span>
            <span className="text-sm font-medium text-gray-200 truncate">
              {best.email}
            </span>
          </div>

          {/* Two-column layout: Session + Weekly */}
          <div className="grid grid-cols-2 gap-x-4">
            <QuotaBar
              label="Session (5h)"
              utilization={best.utilization?.five_hour?.utilization}
              resetsAt={best.utilization?.five_hour?.resets_at}
              defaultReset="5h"
            />
            <QuotaBar
              label="Weekly (7d)"
              utilization={best.utilization?.seven_day?.utilization}
              resetsAt={best.utilization?.seven_day?.resets_at}
              defaultReset="7d"
            />
          </div>
        </div>
      )}
    </div>
  );
}

export default RecommendedAccount;
