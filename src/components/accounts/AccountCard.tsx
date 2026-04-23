import { format } from 'date-fns';
import type { Account } from '../../types/account';
import { getLifecycleStage, getLifecycleColor, getPlanColor } from '../../types/account';

interface AccountCardProps {
  account: Account;
}

function stageBadgeClass(stage: string): string {
  const color = getLifecycleColor(stage);
  if (color === 'green') return 'bg-green-100 dark:bg-green-500/20 text-green-700 dark:text-green-400';
  if (color === 'blue') return 'bg-blue-100 dark:bg-blue-500/20 text-blue-700 dark:text-blue-400';
  if (color === 'orange') return 'bg-orange-100 dark:bg-orange-500/20 text-orange-700 dark:text-orange-400';
  return 'bg-gray-100 dark:bg-gray-500/20 text-gray-600 dark:text-gray-400';
}

function planBadgeClass(plan: string): string {
  const color = getPlanColor(plan);
  if (color === 'purple') return 'bg-purple-100 dark:bg-purple-500/20 text-purple-700 dark:text-purple-400';
  if (color === 'blue') return 'bg-blue-100 dark:bg-blue-500/20 text-blue-700 dark:text-blue-400';
  return 'bg-gray-100 dark:bg-gray-500/20 text-gray-600 dark:text-gray-400';
}

function AccountCard({ account }: AccountCardProps) {
  const stage = getLifecycleStage(account);
  const plan = account.subscriptionType || account.plan || 'free';

  return (
    <div className="bg-white dark:bg-base-100 rounded-xl p-4 border border-gray-100 dark:border-base-200 hover:border-blue-200 dark:hover:border-blue-500/30 transition-colors shadow-sm">
      <div className="flex items-start justify-between mb-3">
        <div className="flex-1 min-w-0">
          <p className="text-sm font-medium text-gray-900 dark:text-base-content truncate">{account.email}</p>
          <p className="text-xs text-gray-400 dark:text-gray-500 mt-0.5">{account.fullName}</p>
        </div>
      </div>

      <div className="flex items-center gap-2 mb-3">
        <span className={`inline-flex items-center px-2 py-0.5 rounded-md text-xs font-medium ${stageBadgeClass(stage)}`}>
          {stage}
        </span>
        <span className={`inline-flex items-center px-2 py-0.5 rounded-md text-xs font-medium ${planBadgeClass(plan)}`}>
          {plan}
        </span>
        <span className="inline-flex items-center px-2 py-0.5 rounded-md text-xs font-medium bg-gray-100 dark:bg-gray-500/20 text-gray-600 dark:text-gray-400">
          {(account.country || account.region || 'us').toUpperCase()}
        </span>
      </div>

      <div className="flex items-center justify-between text-xs text-gray-400 dark:text-gray-500">
        <span>{account.android?.device?.model || 'Unknown'}</span>
        <span>{format(new Date(account.createdAt), 'yyyy-MM-dd')}</span>
      </div>
    </div>
  );
}

export default AccountCard;
