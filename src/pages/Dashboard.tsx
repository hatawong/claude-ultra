import { AlertTriangle, ArrowRight, Download, Plus, RefreshCw, Sparkles, Users, Zap, DollarSign, Shield } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, Link } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { useAccountStore } from '../stores/useAccountStore';
import { useAuthStore } from '../stores/useAuthStore';
import RecommendedAccount from '../components/dashboard/CurrentAccount';
import QuotaRanking from '../components/dashboard/BestAccounts';
import AddAccountDialog from '../components/accounts/AddAccountDialog';
import ConfirmDialog from '../components/common/ConfirmDialog';
import { showToast } from '../components/common/Toast';
import * as gatewayService from '../services/gatewayService';

function Dashboard() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { accounts, fetchAccounts, refreshQuota, loading } = useAccountStore();
  const { authStatus } = useAuthStore();
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [todayRequests, setTodayRequests] = useState(0);
  const [isAddDialogOpen, setIsAddDialogOpen] = useState(false);
  const {
    addingAccountId, setAddingAccountId,
    addingStaticProxy, setAddingStaticProxy,
    addingTestResult, setAddingTestResult,
  } = useAccountStore();
  const [isRefreshConfirmOpen, setIsRefreshConfirmOpen] = useState(false);
  const [proxyMode, setProxyMode] = useState<string>('unknown');

  useEffect(() => {
    invoke<string>('get_proxy_mode').then(setProxyMode).catch(() => {});
  }, []);

  useEffect(() => {
    fetchAccounts();
    const todayStart = new Date();
    todayStart.setHours(0, 0, 0, 0);
    gatewayService.getCostSummary(todayStart.getTime(), Date.now())
      .then(s => setTodayRequests(s.total_requests))
      .catch(() => {});
  }, [fetchAccounts]);

  const activeAccounts = useMemo(
    () => accounts.filter(a => !a.disabled && !a.userDisabled),
    [accounts],
  );

  const stats = useMemo(() => {
    // 5h average utilization
    const fiveHourUtils = activeAccounts
      .map(a => a.utilization?.five_hour?.utilization)
      .filter((u): u is number => u !== null && u !== undefined);
    const avgFiveHour = fiveHourUtils.length > 0
      ? Math.round(fiveHourUtils.reduce((a, b) => a + b, 0) / fiveHourUtils.length)
      : null;

    // 7d average utilization
    const sevenDayUtils = activeAccounts
      .map(a => a.utilization?.seven_day?.utilization)
      .filter((u): u is number => u !== null && u !== undefined);
    const avgSevenDay = sevenDayUtils.length > 0
      ? Math.round(sevenDayUtils.reduce((a, b) => a + b, 0) / sevenDayUtils.length)
      : null;

    // Low quota: 5h utilization > 80%
    const lowQuota = activeAccounts.filter(a => {
      const u = a.utilization?.five_hour?.utilization;
      return u !== null && u !== undefined && u > 80;
    }).length;

    return {
      total: accounts.length,
      avgFiveHour,
      avgSevenDay,
      lowQuota,
    };
  }, [activeAccounts, accounts]);

  const handleRefreshClick = useCallback(() => {
    setIsRefreshConfirmOpen(true);
  }, []);

  const executeRefresh = useCallback(async () => {
    setIsRefreshConfirmOpen(false);
    setIsRefreshing(true);
    try {
      const cliAccounts = accounts.filter(a => a.cli && !a.disabled && a.cli!.expiresAt > Date.now());
      if (cliAccounts.length === 0) {
        await fetchAccounts();
        return;
      }
      const results = await Promise.allSettled(
        cliAccounts.map(a => refreshQuota(a.accountId))
      );
      const failed = results.filter(r => r.status === 'rejected').length;
      if (failed > 0) {
        showToast(`${cliAccounts.length - failed} ${t('common.success', 'success')}, ${failed} ${t('common.failed', 'failed')}`, 'warning');
      }
    } finally {
      setIsRefreshing(false);
    }
  }, [accounts, fetchAccounts, refreshQuota, t]);

  const handleExportAll = useCallback(async () => {
    const json = JSON.stringify(accounts, null, 2);
    try {
      const now = new Date();
      const dateStr = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
      const filePath = await save({
        defaultPath: `claude_ultra_accounts_${dateStr}.json`,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (filePath) {
        await writeTextFile(filePath, json);
        showToast(t('common.success', 'Success'));
      }
    } catch (e) {
      console.error('Export failed:', e);
      showToast(t('dashboard.toast.export_error', 'Export failed'), 'error');
    }
  }, [accounts, t]);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="p-5 space-y-4 max-w-7xl mx-auto" style={{ position: 'relative', zIndex: 1 }}>
        {/* Greeting + action buttons */}
        <div className="flex justify-between items-center">
          <div>
            <h1 className="text-2xl font-bold text-gray-900 dark:text-base-content">
              {authStatus?.mode === 'distribution' && authStatus.user
                ? t('dashboard.hello', 'Hello, User').replace('{{name}}', authStatus.user.displayName).replace('User', authStatus.user.displayName).replace('用户', authStatus.user.displayName)
                : accounts.length > 0
                  ? t('dashboard.hello', 'Hello, User').replace('{{name}}', accounts[0].email.split('@')[0]).replace('User', accounts[0].email.split('@')[0]).replace('用户', accounts[0].email.split('@')[0])
                  : t('dashboard.hello', 'Hello')}
            </h1>
          </div>
          <div className="flex gap-2">
            <button
              className="px-3 py-1.5 bg-gray-700 dark:bg-base-200 text-white dark:text-gray-300 text-xs font-medium rounded-lg hover:bg-gray-800 dark:hover:bg-base-100 transition-colors flex items-center gap-1.5 shadow-sm"
              onClick={() => setIsAddDialogOpen(true)}
            >
              <Plus className="w-3.5 h-3.5" />
              <span className="hidden sm:inline">{t('accounts.add_account', 'Add Account')}</span>
            </button>
            <button
              className={`px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1.5 shadow-sm ${isRefreshing ? 'opacity-70 cursor-not-allowed' : ''}`}
              onClick={handleRefreshClick}
              disabled={isRefreshing || loading}
            >
              <RefreshCw className={`w-3.5 h-3.5 ${isRefreshing ? 'animate-spin' : ''}`} />
              <span className="hidden sm:inline">
                {isRefreshing ? t('dashboard.refreshing', 'Refreshing...') : t('dashboard.refresh_quota', 'Refresh')}
              </span>
            </button>
          </div>
        </div>

        {/* Stats cards — 5 columns */}
        <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
          {/* 1. Total Accounts */}
          <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-blue-50 dark:bg-blue-900/20 rounded-md">
                <Users className="w-4 h-4 text-blue-500 dark:text-blue-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{stats.total}</div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('dashboard.total_accounts', 'Total Accounts')}</div>
          </div>

          {/* 2. Today Requests */}
          <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-purple-50 dark:bg-purple-900/20 rounded-md">
                <Sparkles className="w-4 h-4 text-purple-500 dark:text-purple-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{todayRequests}</div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('dashboard.today_requests', 'Today Requests')}</div>
          </div>

          {/* 3. 5h Avg Utilization */}
          <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-green-50 dark:bg-green-900/20 rounded-md">
                <Zap className="w-4 h-4 text-green-500 dark:text-green-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">
              {stats.avgFiveHour !== null ? `${stats.avgFiveHour}%` : '—'}
            </div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('dashboard.avg_five_hour', '5h Avg Usage')}</div>
            {stats.avgFiveHour !== null && (
              <div className={`text-[10px] mt-1 ${
                stats.avgFiveHour < 50
                  ? 'text-green-600 dark:text-green-400'
                  : 'text-orange-600 dark:text-orange-400'
              }`}>
                {stats.avgFiveHour < 50
                  ? t('dashboard.quota_sufficient', '✓ Quota sufficient')
                  : t('dashboard.quota_low', '⚠️ Quota low')}
              </div>
            )}
          </div>

          {/* 4. 7d Avg Utilization */}
          <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-cyan-50 dark:bg-cyan-900/20 rounded-md">
                <DollarSign className="w-4 h-4 text-cyan-500 dark:text-cyan-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">
              {stats.avgSevenDay !== null ? `${stats.avgSevenDay}%` : '—'}
            </div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('dashboard.avg_seven_day', '7d Avg Usage')}</div>
            {stats.avgSevenDay !== null && (
              <div className={`text-[10px] mt-1 ${
                stats.avgSevenDay < 50
                  ? 'text-green-600 dark:text-green-400'
                  : 'text-orange-600 dark:text-orange-400'
              }`}>
                {stats.avgSevenDay < 50
                  ? t('dashboard.quota_sufficient', '✓ Quota sufficient')
                  : t('dashboard.quota_low', '⚠️ Quota low')}
              </div>
            )}
          </div>

          {/* 5. Low Quota */}
          <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-orange-50 dark:bg-orange-900/20 rounded-md">
                <AlertTriangle className="w-4 h-4 text-orange-500 dark:text-orange-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{stats.lowQuota}</div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('dashboard.low_quota_accounts', 'Low Quota')}</div>
            <div className="text-[10px] text-gray-400 dark:text-gray-500 mt-1">{t('dashboard.usage_above_80', 'Usage > 80%')}</div>
          </div>
        </div>

        {/* Two-column layout: AccountOverview + QuotaRanking */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <RecommendedAccount
            accounts={activeAccounts}
          />
          <QuotaRanking
            accounts={activeAccounts}
          />
        </div>

        {/* Quick links */}
        <div className="grid grid-cols-2 gap-3">
          <button
            className="bg-indigo-50 dark:bg-indigo-900/20 rounded-lg p-3 shadow-sm border border-indigo-100 dark:border-indigo-900/30 hover:border-indigo-300 dark:hover:border-indigo-700 hover:shadow-md transition-all flex items-center justify-between group"
            onClick={() => navigate('/accounts')}
          >
            <span className="text-indigo-700 dark:text-indigo-300 font-medium text-sm">
              {t('dashboard.view_all_accounts', 'View All Accounts')}
            </span>
            <ArrowRight className="w-4 h-4 text-indigo-400 dark:text-indigo-500 group-hover:text-indigo-600 dark:group-hover:text-indigo-300 group-hover:translate-x-1 transition-all" />
          </button>
          <button
            className="bg-purple-50 dark:bg-purple-900/20 rounded-lg p-3 shadow-sm border border-purple-100 dark:border-purple-900/30 hover:border-purple-300 dark:hover:border-purple-700 hover:shadow-md transition-all flex items-center justify-between group"
            onClick={handleExportAll}
          >
            <span className="text-purple-700 dark:text-purple-300 font-medium text-sm">
              {t('dashboard.export_data', 'Export Data')}
            </span>
            <Download className="w-4 h-4 text-purple-400 dark:text-purple-500 group-hover:text-purple-600 dark:group-hover:text-purple-300 transition-all" />
          </button>
        </div>

        {/* Direct mode warning */}
        {proxyMode === 'direct' && (
          <Link
            to="/settings?tab=proxy"
            className="flex items-center gap-2 px-4 py-2.5 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-lg text-sm text-yellow-700 dark:text-yellow-400 hover:bg-yellow-100 dark:hover:bg-yellow-900/30 transition-colors"
          >
            <Shield className="w-4 h-4 shrink-0" />
            <span>{t('proxy.direct_warning')}</span>
          </Link>
        )}
      </div>

      {/* Add Account Dialog */}
      {isAddDialogOpen && (
        <AddAccountDialog
          accountId={addingAccountId}
          onAccountIdChange={setAddingAccountId}
          staticProxy={addingStaticProxy}
          onStaticProxyChange={setAddingStaticProxy}
          testResult={addingTestResult}
          onTestResultChange={setAddingTestResult}
          onClose={() => setIsAddDialogOpen(false)}
          onToast={(msg: string) => showToast(msg)}
        />
      )}

      {isRefreshConfirmOpen && (
        <ConfirmDialog
          title={t('accounts.refresh_confirm_title', 'Refresh Quotas')}
          message={t('accounts.refresh_confirm_all', 'Refresh quota for all {{count}} account(s) with CLI credentials?', { count: accounts.filter(a => a.cli && !a.disabled && a.cli!.expiresAt > Date.now()).length })}
          onConfirm={executeRefresh}
          onCancel={() => setIsRefreshConfirmOpen(false)}
        />
      )}
    </div>
  );
}

export default Dashboard;
