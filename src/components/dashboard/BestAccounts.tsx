import { TrendingUp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Account } from '../../types/account';
import { getPlanLabel, getPlanBadgeClass } from '../../types/account';

interface QuotaRankingProps {
  accounts: Account[];
}

/** Extract multiplier from rateLimitTier (default_claude_max_20x -> 20, default_claude_max_5x -> 5, pro -> 1, free -> 1) */
function getTierMultiplier(account: Account): number {
  const tier = account.rateLimitTier || '';
  const match = tier.match(/(\d+)x$/);
  if (match) return parseInt(match[1]);
  const type = (account.subscriptionType || 'free').toLowerCase();
  if (type === 'max') return 5; // default max without explicit tier
  if (type === 'pro') return 1;
  return 1;
}

/** Weighted remaining quota score = (100 - utilization) * multiplier */
function weightedRemaining(account: Account): number {
  const util = account.utilization?.five_hour?.utilization;
  if (util == null) return -1;
  return (100 - util) * getTierMultiplier(account);
}

function QuotaRanking({ accounts }: QuotaRankingProps) {
  const { t } = useTranslation();

  const ranked = accounts
    .filter(a => weightedRemaining(a) >= 0)
    .sort((a, b) => weightedRemaining(b) - weightedRemaining(a))
    .slice(0, 5);

  return (
    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 h-full flex flex-col">
      <h2 className="text-base font-semibold text-gray-900 dark:text-base-content mb-3 flex items-center gap-2">
        <TrendingUp className="w-4 h-4 text-blue-500 dark:text-blue-400" />
        {t('dashboard.quota_ranking', 'Quota Ranking')}
      </h2>

      <div className="space-y-1.5 flex-1">
        {ranked.length === 0 ? (
          <div className="text-center py-4 text-gray-400 dark:text-gray-500 text-sm">
            {t('dashboard.no_quota_data', 'No quota data. Refresh quota first.')}
          </div>
        ) : (
          ranked.map((account, index) => {
            const util = account.utilization!.five_hour!.utilization!;
            const remaining = Math.round(100 - util);
            const planLabel = getPlanLabel(account);
            const planClass = getPlanBadgeClass(account);
            return (
              <div
                key={account.accountId}
                className="flex items-center gap-2.5 px-2.5 py-2 rounded-lg hover:bg-gray-50 dark:hover:bg-base-200 transition-colors"
              >
                <span className="text-xs font-bold text-gray-400 dark:text-gray-500 w-4 text-right shrink-0">
                  {index + 1}.
                </span>
                <span className={`inline-flex items-center px-1.5 py-0 rounded text-[10px] font-semibold shrink-0 ${planClass}`}>
                  {planLabel}{planLabel === 'Max 5x' ? '\u00a0\u00a0' : ''}
                </span>
                <span className="text-sm text-gray-700 dark:text-gray-300 truncate flex-1 min-w-0">
                  {account.email}
                </span>
                <span className="text-xs text-gray-500 shrink-0">5h</span>
                <span className="text-xs font-bold text-gray-200 shrink-0 w-9 text-right">
                  {remaining}%
                </span>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

export default QuotaRanking;
