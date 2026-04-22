import { useEffect, useMemo, useRef, useState, useCallback } from 'react';
import {
  Download, Plus, RefreshCw, Search, Upload,
  Trash2, ToggleLeft, ToggleRight,
} from 'lucide-react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import {
  DndContext, closestCenter, KeyboardSensor, PointerSensor,
  useSensor, useSensors, type DragEndEvent,
} from '@dnd-kit/core';
import {
  SortableContext, verticalListSortingStrategy, arrayMove,
} from '@dnd-kit/sortable';
import { save, open } from '@tauri-apps/plugin-dialog';
import { writeTextFile, readTextFile } from '@tauri-apps/plugin-fs';

import { useAccountStore } from '../stores/useAccountStore';
import AccountTable from '../components/accounts/AccountTable';
import AccountDetailsDialog from '../components/accounts/AccountDetailsDialog';
import AddAccountDialog from '../components/accounts/AddAccountDialog';
import AccountWebLoginDialog from '../components/accounts/AccountWebLoginDialog';
import AccountWebOAuthDialog from '../components/accounts/AccountWebOAuthDialog';
import ConfirmDialog from '../components/common/ConfirmDialog';
import { showToast } from '../components/common/Toast';
import Pagination from '../components/common/Pagination';
import { cn } from '../utils/cn';
import type { Account, FilterType } from '../types/account';

const FILTER_TABS: { key: FilterType; labelKey: string; fallback: string }[] = [
  { key: 'all', labelKey: 'accounts.all', fallback: 'All' },
  { key: 'free', labelKey: 'accounts.free', fallback: 'Free' },
  { key: 'pro', labelKey: 'accounts.pro', fallback: 'Pro' },
  { key: 'max', labelKey: 'accounts.max', fallback: 'Max' },
];

function Accounts() {
  const { t } = useTranslation();
  const {
    accounts, loading, error, fetchAccounts,
    selectedIds, toggleSelected, selectAll, clearSelection,
    deleteAccount, updateAccountLabel, toggleUserStatus,
    importAccounts, reorderAccounts,
  } = useAccountStore();

  const [searchQuery, setSearchQuery] = useState('');
  const [filter, setFilter] = useState<FilterType>('all');
  const [isSearchExpanded, setIsSearchExpanded] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);

  // Pagination
  const [currentPage, setCurrentPage] = useState(1);
  const [localPageSize, setLocalPageSize] = useState<number | null>(() => {
    const saved = localStorage.getItem('claude_ultra_page_size');
    return saved ? parseInt(saved) : null;
  });
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerSize, setContainerSize] = useState({ width: 0, height: 0 });

  // Dialog state
  const [detailsAccount, setDetailsAccount] = useState<Account | null>(null);
  const [webLoginAccount, setWebLoginAccount] = useState<Account | null>(null);
  const [webOAuthAccount, setOAuthAccount] = useState<Account | null>(null);
  const [runningWebLogins, setRunningWebLogins] = useState<Set<string>>(new Set());

  // Listen to web-login subprocess events
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    const WL = 'web-login-';
    const doneTaskIds = new Set<string>(); // Completed tasks, ignore late events
    const extractId = (tid: string) => tid.startsWith(WL) ? tid.slice(WL.length) : null;
    const markRunning = (tid: string, fromStdout: boolean = false) => {
      if (fromStdout) doneTaskIds.delete(tid); // stdout event means a new run started, clear done mark
      if (doneTaskIds.has(tid)) return;
      const id = extractId(tid);
      if (id) setRunningWebLogins(prev => { const n = new Set(prev); n.add(id); return n; });
    };
    const markDone = (tid: string) => {
      doneTaskIds.add(tid);
      const id = extractId(tid);
      if (id) setRunningWebLogins(prev => { const n = new Set(prev); n.delete(id); return n; });
    };

    let disposed = false;
    const reg = async (promise: Promise<UnlistenFn>) => {
      const fn = await promise;
      if (disposed) fn();
      else unlisteners.push(fn);
    };
    const setup = async () => {
      await reg(listen<any>('subprocess://step', e => { if (e.payload.taskId?.startsWith(WL)) markRunning(e.payload.taskId, true); }));
      await reg(listen<any>('subprocess://log', e => { if (e.payload.taskId?.startsWith(WL)) markRunning(e.payload.taskId, false); }));
      // Fallback: save cookies/proxy/profile even when dialog is closed
      await reg(listen<any>('subprocess://cookies', e => {
        if (!e.payload.taskId?.startsWith(WL)) return;
        const sk = e.payload.data?.sessionKey;
        if (!sk) return;
        const aid = extractId(e.payload.taskId);
        if (aid) invoke('update_web_login', { accountId: aid, cookies: e.payload.data?.cookies || [], sessionKey: sk, proxy: null }).catch(() => {});
      }));
      await reg(listen<any>('subprocess://meta', e => {
        if (!e.payload.taskId?.startsWith(WL)) return;
        const aid = extractId(e.payload.taskId);
        const meta = e.payload.data?.data || e.payload.data;
        if (aid && meta) invoke('update_account_profile', { accountId: aid, email: meta.email || null, accountUuid: meta.accountUuid || null, orgId: meta.orgId || null, fullName: meta.fullName || null, subscriptionType: meta.subscriptionType || null, rateLimitTier: meta.rateLimitTier || null, billingType: meta.billingType || null }).then(() => fetchAccounts()).catch(() => {});
      }));
      await reg(listen<any>('subprocess://result', e => {
        if (!e.payload.taskId?.startsWith(WL)) return;
        markDone(e.payload.taskId);
        if (e.payload.data?.success) {
          const aid = extractId(e.payload.taskId);
          if (aid) {
            invoke('update_web_login', { accountId: aid, cookies: e.payload.data?.data?.cookies || [], sessionKey: e.payload.data?.data?.sessionKey || '', proxy: e.payload.data?.data?.proxy || null }).catch(() => {});
            invoke('update_account_profile', { accountId: aid, email: e.payload.data?.data?.email || null, accountUuid: e.payload.data?.data?.accountUuid || null, orgId: e.payload.data?.data?.orgId || null, fullName: e.payload.data?.data?.fullName || null, subscriptionType: e.payload.data?.data?.subscriptionType || null, rateLimitTier: e.payload.data?.data?.rateLimitTier || null, billingType: e.payload.data?.data?.billingType || null }).catch(() => {});
          }
          fetchAccounts();
        }
      }));
      await reg(listen<any>('subprocess://error', e => {
        if (!e.payload.taskId?.startsWith(WL)) return;
        markDone(e.payload.taskId);
        const msg = (e.payload.data?.msg || '').toLowerCase();
        if (msg.includes('suspended') || msg.includes('banned')) {
          const aid = extractId(e.payload.taskId);
          if (aid) {
            invoke('mark_account_disabled', {
              accountId: aid,
              reason: `Web login banned (${new Date().toISOString().slice(0, 10)})`,
            }).then(() => fetchAccounts()).catch(() => {});
          }
        }
      }));
      await reg(listen<any>('subprocess://completed', e => { if (e.payload.taskId?.startsWith(WL)) { markDone(e.payload.taskId); fetchAccounts(); } }));
      await reg(listen<any>('subprocess://failed', e => { if (e.payload.taskId?.startsWith(WL)) markDone(e.payload.taskId); }));
    };
    setup();
    return () => { disposed = true; unlisteners.forEach(fn => fn()); };
  }, []);

  const [confirmAction, setConfirmAction] = useState<{ title: string; message: string; onConfirm: () => void } | null>(null);
  const [editingLabel, setEditingLabel] = useState<{ account: Account; value: string } | null>(null);
  // Quota - from store (persists across tab switches)
  const { refreshQuota, refreshingQuota } = useAccountStore();
  const [isRefreshConfirmOpen, setIsRefreshConfirmOpen] = useState(false);
  const [isAddDialogOpen, setIsAddDialogOpen] = useState(false);
  const { addingAccountId, setAddingAccountId } = useAccountStore();

  const handleRefreshSingle = useCallback(async (accountId: string) => {
    try {
      await refreshQuota(accountId);
    } catch (e) {
      showToast(`${t('common.error', 'Error')}: ${String(e).substring(0, 80)}`, 'error');
    }
  }, [refreshQuota, t]);

  const handleRefreshClick = useCallback(() => {
    setIsRefreshConfirmOpen(true);
  }, []);

  const executeRefresh = useCallback(async () => {
    setIsRefreshConfirmOpen(false);

    const isBatch = selectedIds.size > 0;
    const canRefresh = (a: Account) => a.cli && !a.disabled && a.cli.expiresAt > Date.now();
    const targetIds = isBatch
      ? Array.from(selectedIds).filter(id => { const a = accounts.find(x => x.accountId === id); return a && canRefresh(a); })
      : accounts.filter(canRefresh).map(a => a.accountId);

    if (targetIds.length === 0) {
      showToast(t('accounts.no_cli_accounts', 'No accounts with CLI credentials'), 'warning');
      return;
    }

    const results = await Promise.allSettled(
      targetIds.map(id => refreshQuota(id))
    );

    let failedCount = 0;
    let successCount = 0;
    results.forEach(r => {
      if (r.status === 'fulfilled') successCount++;
      else failedCount++;
    });

    if (failedCount > 0) {
      showToast(`${successCount} ${t('common.success', 'success')}, ${failedCount} ${t('common.failed', 'failed')}`, failedCount === targetIds.length ? 'error' : 'warning');
    }
  }, [selectedIds, accounts, refreshQuota, t]);

  // DnD sensors
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor),
  );

  useEffect(() => { fetchAccounts(); }, [fetchAccounts]);

  useEffect(() => {
    if (localPageSize !== null) {
      localStorage.setItem('claude_ultra_page_size', localPageSize.toString());
    }
  }, [localPageSize]);

  useEffect(() => {
    if (!containerRef.current) return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setContainerSize({ width: entry.contentRect.width, height: entry.contentRect.height });
      }
    });
    observer.observe(containerRef.current);
    return () => observer.disconnect();
  }, []);

  useEffect(() => { setCurrentPage(1); }, [filter, searchQuery]);

  const ITEMS_PER_PAGE = useMemo(() => {
    if (localPageSize && localPageSize > 0) return localPageSize;
    if (!containerSize.height) return 10;
    const headerHeight = 36;
    const rowHeight = 56;
    return Math.max(10, Math.floor((containerSize.height - headerHeight) / rowHeight));
  }, [localPageSize, containerSize]);

  // Filtering
  const searchedAccounts = useMemo(() => {
    if (!searchQuery) return accounts;
    const q = searchQuery.toLowerCase();
    return accounts.filter((a) =>
      a.email.toLowerCase().includes(q) ||
      (a.customLabel && a.customLabel.toLowerCase().includes(q))
    );
  }, [accounts, searchQuery]);

  const filterCounts = useMemo(() => {
    const getType = (a: Account) => (a.subscriptionType || a.plan || 'free').toLowerCase();
    return {
      all: searchedAccounts.length,
      pro: searchedAccounts.filter((a) => getType(a) === 'pro').length,
      max: searchedAccounts.filter((a) => getType(a) === 'max').length,
      free: searchedAccounts.filter((a) => getType(a) === 'free' || !getType(a)).length,
    };
  }, [searchedAccounts]);

  const filteredAccounts = useMemo(() => {
    if (filter === 'all') return searchedAccounts;
    return searchedAccounts.filter((a) => {
      const type = (a.subscriptionType || a.plan || 'free').toLowerCase();
      return type === filter;
    });
  }, [searchedAccounts, filter]);

  const paginatedAccounts = useMemo(() => {
    const start = (currentPage - 1) * ITEMS_PER_PAGE;
    return filteredAccounts.slice(start, start + ITEMS_PER_PAGE);
  }, [filteredAccounts, currentPage, ITEMS_PER_PAGE]);

  // Handlers
  const handleDelete = useCallback((account: Account) => {
    setConfirmAction({
      title: t('accounts.dialog.delete_title', 'Delete Confirmation'),
      message: t('accounts.dialog.delete_msg', 'Are you sure you want to delete this account? This action cannot be undone.'),
      onConfirm: async () => {
        try {
          await deleteAccount(account.accountId);
          showToast(t('common.delete_success', 'Deleted'));
        } catch (e) {
          showToast(String(e), "error");
        }
        setConfirmAction(null);
      },
    });
  }, [deleteAccount, showToast, t]);

  const handleBatchDelete = useCallback(() => {
    const count = selectedIds.size;
    setConfirmAction({
      title: t('accounts.dialog.batch_delete_title', 'Batch Delete'),
      message: t('accounts.dialog.batch_delete_msg', { count }),
      onConfirm: async () => {
        let ok = 0; let fail = 0;
        for (const id of selectedIds) {
          try { await deleteAccount(id); ok++; } catch { fail++; }
        }
        clearSelection();
        showToast(fail > 0 ? `Deleted ${ok}, failed ${fail}` : t('common.delete_success', 'Deleted'));
        setConfirmAction(null);
      },
    });
  }, [selectedIds, deleteAccount, clearSelection, showToast, t]);

  const handleToggleProxy = useCallback(async (account: Account) => {
    try {
      await toggleUserStatus(account.accountId, account.userDisabled);
      showToast(account.userDisabled ? t('accounts.enable_user', 'Enabled') : t('accounts.disable_user', 'Disabled'));
    } catch (e) {
      showToast(String(e), "error");
    }
  }, [toggleUserStatus, showToast, t]);

  const handleEditLabel = useCallback((account: Account) => {
    setEditingLabel({ account, value: account.customLabel || '' });
  }, []);

  const handleLabelSave = useCallback(async () => {
    if (!editingLabel) return;
    try {
      await updateAccountLabel(editingLabel.account.accountId, editingLabel.value);
      showToast(t('accounts.label_updated', 'Label updated'));
    } catch (e) {
      showToast(String(e), "error");
    }
    setEditingLabel(null);
  }, [editingLabel, updateAccountLabel, showToast, t]);

  const handleExportSingle = useCallback(async (account: Account) => {
    const json = JSON.stringify([account], null, 2);
    const path = await save({
      defaultPath: `claude_ultra_${account.accountId}.json`,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (path) {
      await writeTextFile(path, json);
      showToast(t('common.success', 'Success'));
    }
  }, [showToast, t]);

  const handleExportAll = useCallback(async () => {
    const toExport = selectedIds.size > 0
      ? accounts.filter((a) => selectedIds.has(a.accountId))
      : accounts;
    const json = JSON.stringify(toExport, null, 2);
    const now = new Date();
    const dateStr = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
    const path = await save({
      defaultPath: `claude_ultra_accounts_${dateStr}.json`,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (path) {
      await writeTextFile(path, json);
      showToast(t('common.success', 'Success'));
    }
  }, [accounts, selectedIds, showToast, t]);

  const handleImport = useCallback(async () => {
    const path = await open({
      filters: [{ name: 'JSON', extensions: ['json'] }],
      multiple: false,
    });
    if (!path) return;
    try {
      const content = await readTextFile(path as string);
      const result = await importAccounts(content);
      showToast(`${t('accounts.import_success', 'Imported')}: ${result.success} ${t('common.success', 'success')}, ${result.skipped} ${t('accounts.import_skipped', 'skipped')}, ${result.failed} ${t('accounts.import_failed', 'failed')}`);
    } catch (e) {
      showToast(String(e), "error");
    }
  }, [importAccounts, showToast, t]);

  const isDragEnabled = filter === 'all' && !searchQuery;

  const handleDragEnd = useCallback((event: DragEndEvent) => {
    if (!isDragEnabled) return;
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = accounts.findIndex((a) => a.accountId === active.id);
    const newIndex = accounts.findIndex((a) => a.accountId === over.id);
    if (oldIndex === -1 || newIndex === -1) return;
    const newOrder = arrayMove(accounts, oldIndex, newIndex);
    reorderAccounts(newOrder.map((a) => a.accountId));
  }, [isDragEnabled, accounts, reorderAccounts]);

  const handleBatchToggle = useCallback(async (enable: boolean) => {
    const ids = Array.from(selectedIds);
    const results = await Promise.allSettled(
      ids.map(id => toggleUserStatus(id, enable))
    );
    const ok = results.filter(r => r.status === 'fulfilled').length;
    const fail = results.filter(r => r.status === 'rejected').length;
    clearSelection();
    const msg = enable
      ? t('accounts.toast.proxy_enabled', { count: ok })
      : t('accounts.toast.user_disabled', { count: ok });
    if (fail === 0) {
      showToast(msg, 'success');
    } else {
      showToast(`${msg}, ${fail} ${t('common.failed', 'failed')}`, 'warning');
    }
  }, [selectedIds, toggleUserStatus, clearSelection, t]);

  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center">
          <p className="text-red-400 mb-4">{error}</p>
          <button onClick={fetchAccounts} className="px-4 py-2 bg-blue-500 text-white text-sm rounded-lg hover:bg-blue-600">
            {t('common.retry', 'Retry')}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col p-5 gap-4 max-w-7xl mx-auto w-full">
      {/* Toolbar */}
      <div className="flex-none flex items-center gap-2 flex-wrap">
        {/* Search — desktop */}
        <div className="hidden lg:block flex-none w-40 relative transition-all focus-within:w-48">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-gray-400" />
          <input
            type="text"
            placeholder={t('accounts.search_placeholder', 'Search email...')}
            className="w-full pl-9 pr-4 py-2 bg-white dark:bg-base-100 text-sm text-gray-900 dark:text-base-content border border-gray-200 dark:border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent placeholder:text-gray-400 dark:placeholder:text-gray-500"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
        </div>

        {/* Search — mobile */}
        <div className="lg:hidden relative">
          {!isSearchExpanded ? (
            <button
              onClick={() => { setIsSearchExpanded(true); setTimeout(() => searchInputRef.current?.focus(), 100); }}
              className="p-2 bg-gray-100 dark:bg-base-200 hover:bg-gray-200 dark:hover:bg-base-100 rounded-lg transition-colors"
              title={t('accounts.search_placeholder', 'Search')}
            >
              <Search className="w-4 h-4 text-gray-600 dark:text-gray-300" />
            </button>
          ) : (
            <div className="absolute left-0 top-0 z-10 w-64 flex items-center gap-1">
              <div className="flex-1 relative">
                <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-gray-400" />
                <input
                  ref={searchInputRef}
                  type="text"
                  placeholder={t('accounts.search_placeholder', 'Search email...')}
                  className="w-full pl-9 pr-4 py-2 bg-white dark:bg-base-100 text-sm text-gray-900 dark:text-base-content border border-gray-200 dark:border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 shadow-lg"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  onBlur={() => setIsSearchExpanded(false)}
                />
              </div>
            </div>
          )}
        </div>

        {/* Filter tabs */}
        <div className="flex gap-0.5 bg-gray-100/80 dark:bg-base-200 p-1 rounded-xl border border-gray-200/50 dark:border-white/5 shrink-0">
          {FILTER_TABS.map((tab) => (
            <button
              key={tab.key}
              className={cn(
                'px-2 md:px-3 py-1.5 rounded-lg text-[11px] font-semibold transition-all flex items-center gap-1 md:gap-1.5 whitespace-nowrap shrink-0',
                filter === tab.key
                  ? 'bg-white dark:bg-base-100 text-blue-600 dark:text-blue-400 shadow-sm ring-1 ring-black/5'
                  : 'text-gray-500 dark:text-gray-400 hover:text-gray-900 dark:hover:text-base-content hover:bg-white/40',
              )}
              onClick={() => setFilter(tab.key)}
            >
              <span className="hidden md:inline">{t(tab.labelKey, tab.fallback)}</span>
              <span className={cn(
                'px-1.5 py-0.5 rounded-md text-[10px] font-bold transition-colors',
                filter === tab.key
                  ? 'bg-blue-100 dark:bg-blue-500/20 text-blue-600 dark:text-blue-400'
                  : 'bg-gray-200 dark:bg-gray-700 text-gray-500 dark:text-gray-400',
              )}>
                {filterCounts[tab.key]}
              </span>
            </button>
          ))}
        </div>

        <div className="flex-1 min-w-[8px]" />

        {/* Action buttons */}
        <div className="flex items-center gap-1.5 shrink-0">
          {/* + Add (placeholder) */}
          <button
            className="px-2.5 py-2 bg-gray-700 dark:bg-base-200 text-white dark:text-gray-300 text-xs font-medium rounded-lg hover:bg-gray-800 dark:hover:bg-base-100 transition-colors flex items-center gap-1.5 shadow-sm"
            onClick={() => setIsAddDialogOpen(true)}
            title={t('accounts.add_account', 'Add Account')}
          >
            <Plus className="w-3.5 h-3.5" />
          </button>

          {/* Batch actions (appear when selected) */}
          {selectedIds.size > 0 && (
            <>
              <button
                className="px-2.5 py-2 bg-red-500 text-white text-xs font-medium rounded-lg hover:bg-red-600 transition-colors flex items-center gap-1.5 shadow-sm"
                onClick={handleBatchDelete}
                title={t('accounts.delete_selected', { count: selectedIds.size })}
              >
                <Trash2 className="w-3.5 h-3.5" />
                <span className="hidden xl:inline">{t('accounts.delete_selected', { count: selectedIds.size })}</span>
              </button>
              <button
                className="px-2.5 py-2 bg-orange-500 text-white text-xs font-medium rounded-lg hover:bg-orange-600 transition-colors flex items-center gap-1.5 shadow-sm"
                onClick={() => handleBatchToggle(false)}
                title={t('accounts.disable_user_selected', { count: selectedIds.size })}
              >
                <ToggleLeft className="w-3.5 h-3.5" />
                <span className="hidden xl:inline">{t('accounts.disable_user_selected', { count: selectedIds.size })}</span>
              </button>
              <button
                className="px-2.5 py-2 bg-green-500 text-white text-xs font-medium rounded-lg hover:bg-green-600 transition-colors flex items-center gap-1.5 shadow-sm"
                onClick={() => handleBatchToggle(true)}
                title={t('accounts.enable_user_selected', { count: selectedIds.size })}
              >
                <ToggleRight className="w-3.5 h-3.5" />
                <span className="hidden xl:inline">{t('accounts.enable_user_selected', { count: selectedIds.size })}</span>
              </button>
            </>
          )}

          {/* Refresh All Quotas */}
          <button
            className={cn(
              "px-2.5 py-2 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1.5 shadow-sm",
              refreshingQuota.size > 0 && "opacity-70 cursor-not-allowed"
            )}
            onClick={handleRefreshClick}
            disabled={refreshingQuota.size > 0}
            title={selectedIds.size > 0
              ? t('accounts.refresh_selected', 'Refresh Selected')
              : t('accounts.refresh_all', 'Refresh All')
            }
          >
            <RefreshCw className={cn("w-3.5 h-3.5", refreshingQuota.size > 0 && "animate-spin")} />
            {refreshingQuota.size > 0 && (
              <span className="text-[10px]">{refreshingQuota.size}</span>
            )}
          </button>

          {/* Separator */}
          <div className="w-px h-4 bg-gray-200 dark:bg-gray-700 self-center mx-1 shrink-0" />

          {/* Import */}
          <button
            className="px-2.5 py-2 border border-gray-200 dark:border-base-300 text-gray-700 dark:text-gray-300 text-xs font-medium rounded-lg hover:bg-gray-50 dark:hover:bg-base-200 transition-colors flex items-center gap-1.5"
            onClick={handleImport}
            title={t('accounts.import_json', 'Import')}
          >
            <Upload className="w-3.5 h-3.5" />
          </button>

          {/* Export */}
          <button
            className="px-2.5 py-2 border border-gray-200 dark:border-base-300 text-gray-700 dark:text-gray-300 text-xs font-medium rounded-lg hover:bg-gray-50 dark:hover:bg-base-200 transition-colors flex items-center gap-1.5"
            onClick={handleExportAll}
            title={selectedIds.size > 0
              ? t('accounts.export_selected', { count: selectedIds.size })
              : t('common.export', 'Export')
            }
          >
            <Download className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Content area */}
      <div className="flex-1 min-h-0 relative" ref={containerRef}>
        <div className="h-full bg-white dark:bg-base-100 rounded-lg shadow-sm border border-gray-100 dark:border-base-200 flex flex-col overflow-hidden">
          <div className="flex-1 overflow-y-auto">
            <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
              <SortableContext items={paginatedAccounts.map((a) => a.accountId)} strategy={verticalListSortingStrategy}>
                <AccountTable
                  accounts={paginatedAccounts}
                  loading={loading}
                  selectedIds={selectedIds}
                  onToggleSelect={toggleSelected}
                  onSelectAll={selectAll}
                  onClearSelection={clearSelection}
                  onViewDetails={(a) => setDetailsAccount(a)}
                  onEditLabel={handleEditLabel}
                  onExportSingle={handleExportSingle}
                  onDelete={handleDelete}
                  onToggleProxy={handleToggleProxy}
                  onWebLogin={(a) => setWebLoginAccount(a)}
                  onWebOAuth={(a) => setOAuthAccount(a)}
                  runningWebLogins={runningWebLogins}
                  onToast={(msg: string) => showToast(msg)}
                  refreshingQuota={refreshingQuota}
                  onRefreshQuota={handleRefreshSingle}
                />
              </SortableContext>
            </DndContext>
          </div>
        </div>
      </div>

      {/* Pagination */}
      {filteredAccounts.length > 0 && (
        <div className="flex-none">
          <Pagination
            currentPage={currentPage}
            totalPages={Math.ceil(filteredAccounts.length / ITEMS_PER_PAGE)}
            onPageChange={setCurrentPage}
            totalItems={filteredAccounts.length}
            itemsPerPage={ITEMS_PER_PAGE}
            selectedCount={selectedIds.size}
            onPageSizeChange={(newSize) => { setLocalPageSize(newSize); setCurrentPage(1); }}
            pageSizeOptions={[50, 100, 200]}
          />
        </div>
      )}

      {/* Add Account Dialog */}
      {isAddDialogOpen && (
        <AddAccountDialog
          accountId={addingAccountId}
          onAccountIdChange={setAddingAccountId}
          onClose={() => setIsAddDialogOpen(false)}
          onToast={(msg: string) => showToast(msg)}
        />
      )}

      {/* Details Dialog */}
      {detailsAccount && (
        <AccountDetailsDialog
          account={detailsAccount}
          onClose={() => setDetailsAccount(null)}
          onDelete={(a) => { setDetailsAccount(null); handleDelete(a); }}
          onToggleProxy={async (a) => { await handleToggleProxy(a); setDetailsAccount(null); }}
          onToast={(msg: string) => showToast(msg)}
        />
      )}

      {/* Web Login Dialog */}
      {webLoginAccount && (
        <AccountWebLoginDialog
          account={webLoginAccount}
          onClose={() => setWebLoginAccount(null)}
          onDone={() => { fetchAccounts(); }}
        />
      )}

      {/* OAuth Dialog */}
      {webOAuthAccount && (
        <AccountWebOAuthDialog
          account={webOAuthAccount}
          onClose={() => setOAuthAccount(null)}
          onDone={() => { fetchAccounts(); }}
        />
      )}

      {/* Confirm Dialog */}
      {confirmAction && (
        <ConfirmDialog
          title={confirmAction.title}
          message={confirmAction.message}
          onConfirm={confirmAction.onConfirm}
          onCancel={() => setConfirmAction(null)}
        />
      )}

      {/* Refresh Confirm Dialog */}
      {isRefreshConfirmOpen && (
        <ConfirmDialog
          title={t('accounts.refresh_confirm_title', 'Refresh Quotas')}
          message={selectedIds.size > 0
            ? t('accounts.refresh_confirm_selected', 'Refresh quota for {{count}} selected account(s)?', { count: selectedIds.size })
            : t('accounts.refresh_confirm_all', 'Refresh quota for all {{count}} account(s) with CLI credentials?', { count: accounts.filter(a => a.cli && !a.disabled && a.cli!.expiresAt > Date.now()).length })
          }
          onConfirm={executeRefresh}
          onCancel={() => setIsRefreshConfirmOpen(false)}
        />
      )}

      {/* Label Edit Dialog */}
      {editingLabel && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setEditingLabel(null)}>
          <div className="bg-white dark:bg-base-100 rounded-xl shadow-2xl w-[400px] p-6 border border-gray-200 dark:border-base-300" onClick={(e) => e.stopPropagation()}>
            <h3 className="text-sm font-semibold text-gray-900 dark:text-base-content mb-3">
              {t('accounts.edit_label', 'Edit Label')}
            </h3>
            <input
              type="text"
              className="w-full px-3 py-2 text-sm bg-gray-50 dark:bg-base-200 border border-gray-200 dark:border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 text-gray-900 dark:text-base-content"
              placeholder={t('accounts.custom_label_placeholder', 'Enter custom label')}
              maxLength={15}
              value={editingLabel.value}
              onChange={(e) => setEditingLabel({ ...editingLabel, value: e.target.value })}
              onKeyDown={(e) => { if (e.key === 'Enter') handleLabelSave(); if (e.key === 'Escape') setEditingLabel(null); }}
              autoFocus
            />
            <p className="text-[10px] text-gray-400 mt-1">{editingLabel.value.length}/15</p>
            <div className="flex justify-end gap-2 mt-4">
              <button onClick={() => setEditingLabel(null)} className="px-4 py-1.5 text-xs font-medium bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 rounded-lg hover:bg-gray-200 dark:hover:bg-base-300 transition-colors">
                {t('common.cancel', 'Cancel')}
              </button>
              <button onClick={handleLabelSave} className="px-4 py-1.5 text-xs font-medium bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors">
                {t('common.save', 'Save')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Toast is global — rendered in App.tsx via ToastContainer */}
    </div>
  );
}

export default Accounts;
