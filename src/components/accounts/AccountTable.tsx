import React, { useState, useRef } from 'react';
import {
  GripVertical, Info, Tag, Download, Trash2,
  Key, Globe, RefreshCw, ToggleLeft, ToggleRight,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import type { Account, Utilization } from '../../types/account';
import { QuotaItem } from './QuotaDisplay';
import {
  getPlanLabel, getPlanBadgeClass, getAnomalyLabel,
  getLastActivity,
} from '../../types/account';
import { cn } from '../../utils/cn';

interface AccountTableProps {
  accounts: Account[];
  loading: boolean;
  selectedIds: Set<string>;
  onToggleSelect: (id: string) => void;
  onSelectAll: (ids: string[]) => void;
  onClearSelection: () => void;
  onViewDetails: (account: Account) => void;
  onEditLabel: (account: Account) => void;
  onExportSingle: (account: Account) => void;
  onDelete: (account: Account) => void;
  onToggleProxy: (account: Account) => void;
  onWebLogin: (account: Account) => void;
  onWebOAuth: (account: Account) => void;
  runningWebLogins?: Set<string>;
  onToast: (msg: string) => void;
  refreshingQuota?: Set<string>;
  onRefreshQuota?: (accountId: string) => void;
}

function AccountTable({
  accounts, loading, selectedIds,
  onToggleSelect, onSelectAll, onClearSelection,
  onViewDetails, onEditLabel, onExportSingle, onDelete,
  onToggleProxy, onWebLogin, onWebOAuth, runningWebLogins, onToast, refreshingQuota, onRefreshQuota,
}: AccountTableProps) {
  const { t } = useTranslation();

  if (loading && accounts.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center py-20">
        <div className="text-sm text-gray-400">{t('common.loading', 'Loading...')}</div>
      </div>
    );
  }

  const allSelected = accounts.length > 0 && accounts.every((a) => selectedIds.has(a.accountId));

  return (
    <div className="overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="border-b border-gray-100 dark:border-base-200 bg-gray-50 dark:bg-base-200">
            {/* Drag handle */}
            <th className="w-[32px] px-1" />
            {/* Checkbox */}
            <th className="w-[40px] px-2 py-2 text-left">
              <input
                style={{ transform: 'translateY(2px)' }}
                type="checkbox"
                className="checkbox checkbox-xs rounded border-2 border-gray-400 dark:border-gray-500 checked:border-blue-600 checked:bg-blue-600 [--chkbg:theme(colors.blue.600)] [--chkfg:white]"
                checked={allSelected}
                onChange={() => {
                  if (allSelected) onClearSelection();
                  else onSelectAll(accounts.map((a) => a.accountId));
                }}
              />
            </th>
            <th className="px-4 py-2 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider w-[220px]">
              {t('accounts.table.email', 'Email')}
            </th>
            <th className="px-3 py-2 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider w-[340px]">
              {t('accounts.table.quota', 'Quota')}
            </th>
            <th className="px-3 py-2 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider w-[80px]">
              {t('accounts.table.proxy', 'Proxy')}
            </th>
            <th className="px-3 py-2 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider w-[90px]">
              {t('accounts.table.last_activity', 'Last Activity')}
            </th>
            <th className="px-3 py-2 text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider w-[140px] text-center sticky right-0 bg-gray-50 dark:bg-base-200 z-20 shadow-[-12px_0_12px_-12px_rgba(0,0,0,0.1)] dark:shadow-[-12px_0_12px_-12px_rgba(255,255,255,0.05)]">
              {t('accounts.table.actions', 'Actions')}
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-100 dark:divide-base-200">
          {accounts.map((account) => (
            <AccountRow
              key={account.accountId}
              account={account}
              selected={selectedIds.has(account.accountId)}
              onToggleSelect={() => onToggleSelect(account.accountId)}
              onViewDetails={() => onViewDetails(account)}
              onEditLabel={() => onEditLabel(account)}
              onExportSingle={() => onExportSingle(account)}
              onDelete={() => onDelete(account)}
              onToggleProxy={() => onToggleProxy(account)}
              onWebLogin={() => onWebLogin(account)}
              onWebOAuth={() => onWebOAuth(account)}
              isWebLoginRunning={runningWebLogins?.has(account.accountId)}
              onToast={onToast}
              quota={account.utilization}
              isRefreshing={refreshingQuota?.has(account.accountId)}
              onRefreshQuota={() => onRefreshQuota?.(account.accountId)}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ─── AccountRow ─────────────────────────────────────────────

interface AccountRowProps {
  account: Account;
  selected: boolean;
  onToggleSelect: () => void;
  onViewDetails: () => void;
  onEditLabel: () => void;
  onExportSingle: () => void;
  onDelete: () => void;
  onToggleProxy: () => void;
  onWebLogin: () => void;
  onWebOAuth: () => void;
  isWebLoginRunning?: boolean;
  onToast: (msg: string) => void;
  quota?: Utilization | null;
  isRefreshing?: boolean;
  onRefreshQuota?: () => void;
}

function AccountRow({
  account, selected, onToggleSelect,
  onViewDetails, onEditLabel, onExportSingle, onDelete,
  onToggleProxy, onWebLogin, onWebOAuth, isWebLoginRunning, onToast: _onToast, quota, isRefreshing, onRefreshQuota,
}: AccountRowProps) {
  const { t } = useTranslation();
  const planLabel = getPlanLabel(account);
  const planClass = getPlanBadgeClass(account);
  const anomaly = getAnomalyLabel(account);
  const lastActivity = getLastActivity(account);

  const {
    attributes, listeners, setNodeRef, transform, transition, isDragging,
  } = useSortable({ id: account.accountId });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };


  return (
    <tr
      ref={setNodeRef}
      style={style}
      className={cn(
        'group hover:bg-gray-50 dark:hover:bg-base-200/50 transition-colors',
        selected && 'bg-blue-50/30 dark:bg-blue-900/10',
        (account.disabled || account.userDisabled) && 'opacity-70',
      )}
    >
      {/* Col 0: Drag handle */}
      <td className="px-1">
        <button
          {...attributes}
          {...listeners}
          className="p-1 cursor-grab text-gray-300 dark:text-gray-600 hover:text-gray-500 dark:hover:text-gray-400 active:cursor-grabbing"
          title={t('accounts.drag_to_reorder', 'Drag to reorder')}
        >
          <GripVertical className="w-3.5 h-3.5" />
        </button>
      </td>

      {/* Col 1: Checkbox */}
      <td className="w-[40px] px-2 py-2">
        <input
          style={{ transform: 'translateY(2px)' }}
          type="checkbox"
          className="checkbox checkbox-xs rounded border-2 border-gray-400 dark:border-gray-500 checked:border-blue-600 checked:bg-blue-600 [--chkbg:theme(colors.blue.600)] [--chkfg:white]"
          checked={selected}
          onChange={onToggleSelect}
          onClick={(e) => e.stopPropagation()}
        />
      </td>

      {/* Col 2: Email + labels */}
      <td className="px-4 py-2">
        <div>
          <span className="text-sm font-medium text-gray-900 dark:text-base-content truncate block max-w-[260px]">
            {account.email}
          </span>
          <div className="flex items-center gap-1 mt-1.5 flex-wrap">
            <span className={cn('inline-flex items-center px-1.5 py-0 rounded text-[10px] font-semibold', planClass)}>
              {planLabel}
            </span>
            {anomaly && (
              <span className={cn('inline-flex items-center px-1.5 py-0 rounded text-[10px] font-semibold', anomaly.color)}>
                {anomaly.text}
              </span>
            )}
            {account.customLabel && (
              <span className="inline-flex items-center px-1.5 py-0 rounded text-[10px] font-semibold bg-orange-100 dark:bg-orange-500/20 text-orange-700 dark:text-orange-400">
                {account.customLabel}
              </span>
            )}
          </div>
        </div>
      </td>

      {/* Col 3: Quota */}
      <td className="px-3 py-2">
        <QuotaItem usage={quota} />
      </td>

      {/* Col 4: Proxy */}
      <td className="px-3 py-2">
        {account.proxy ? (
          <ProxyDot proxy={account.proxy} t={t} />
        ) : (
          <span className="inline-flex items-center justify-center w-6 h-6">
            <span className="inline-block w-2 h-2 rounded-full bg-gray-300 dark:bg-gray-600" />
          </span>
        )}
      </td>

      {/* Col 5: Last Activity — 2 rows: date + time */}
      <td className="px-3 py-2">
        {lastActivity ? (
          <div className="flex flex-col">
            <span className="text-xs font-medium text-gray-600 dark:text-gray-400 whitespace-nowrap">
              {new Date(lastActivity).toLocaleDateString()}
            </span>
            <span className="text-[10px] text-gray-400 dark:text-gray-500 whitespace-nowrap leading-tight">
              {new Date(lastActivity).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
            </span>
          </div>
        ) : (
          <span className="text-xs text-gray-400 dark:text-gray-500">&mdash;</span>
        )}
      </td>

      {/* Col 6: Actions (two rows) */}
      <td className="px-3 py-1 sticky right-0 bg-white dark:bg-base-100 z-20 shadow-[-12px_0_12px_-12px_rgba(0,0,0,0.1)] dark:shadow-[-12px_0_12px_-12px_rgba(255,255,255,0.05)]">
        <div className="flex flex-col gap-0.5 opacity-60 group-hover:opacity-100 transition-opacity">
          {/* Row 1: Data ops */}
          <div className="flex items-center gap-0.5">
            <ActionBtn icon={Info} title={t('common.details', 'Details')} onClick={onViewDetails} hoverColor="sky" />
            <ActionBtn icon={Tag} title={t('accounts.edit_label', 'Edit Label')} onClick={onEditLabel} hoverColor="indigo" />
            <ActionBtn icon={Download} title={t('common.export', 'Export')} onClick={onExportSingle} hoverColor="indigo" />
            <ActionBtn icon={Trash2} title={t('common.delete', 'Delete')} onClick={onDelete} hoverColor="red" />
          </div>
          {/* Row 2: Functional ops */}
          <div className="flex items-center gap-0.5">
            <ActionBtn
              icon={Key}
              title={account.cli && account.cli.expiresAt < Date.now() ? 'OAuth (Token Expired)' : 'OAuth'}
              onClick={onWebOAuth}
              hoverColor="amber"
              urgent={!!(account.cli && account.cli.expiresAt < Date.now())}
            />
            <ActionBtn
              icon={Globe}
              title={t('task.webLogin', 'Web Login')}
              onClick={onWebLogin}
              hoverColor="blue"
              active={isWebLoginRunning}
            />
            <ActionBtn
              icon={RefreshCw}
              title={t('common.refresh', 'Refresh Quota')}
              onClick={() => onRefreshQuota?.()}
              hoverColor="green"
              spinning={isRefreshing}
            />
            <ActionBtn
              icon={account.userDisabled ? ToggleRight : ToggleLeft}
              title={account.userDisabled ? t('accounts.enable_user', 'Enable') : t('accounts.disable_user', 'Disable')}
              onClick={onToggleProxy}
              hoverColor={account.userDisabled ? 'green' : 'orange'}
            />
          </div>
        </div>
      </td>
    </tr>
  );
}

// ─── ActionBtn ──────────────────────────────────────────────

const hoverColors: Record<string, string> = {
  sky: 'hover:text-sky-600 dark:hover:text-sky-400 hover:bg-sky-50 dark:hover:bg-sky-900/30',
  indigo: 'hover:text-indigo-600 dark:hover:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-900/30',
  red: 'hover:text-red-600 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/30',
  green: 'hover:text-green-600 dark:hover:text-green-400 hover:bg-green-50 dark:hover:bg-green-900/30',
  orange: 'hover:text-orange-600 dark:hover:text-orange-400 hover:bg-orange-50 dark:hover:bg-orange-900/30',
  amber: 'hover:text-amber-600 dark:hover:text-amber-400 hover:bg-amber-50 dark:hover:bg-amber-900/30',
  blue: 'hover:text-blue-600 dark:hover:text-blue-400 hover:bg-blue-50 dark:hover:bg-blue-900/30',
};

// ─── Proxy Dot with instant tooltip ────────────────────────────

function ProxyDot({ proxy, t }: { proxy: NonNullable<Account['proxy']>; t: (key: string, fallback: string) => string }) {
  const [show, setShow] = useState(false);
  const [dropDown, setDropDown] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  return (
    <span
      ref={ref}
      className="relative inline-flex items-center justify-center w-6 h-6 cursor-default"
      onMouseEnter={() => {
        clearTimeout(hideTimer.current);
        if (ref.current) {
          const rect = ref.current.getBoundingClientRect();
          setDropDown(rect.top < 200);
        }
        setShow(true);
      }}
      onMouseLeave={() => { hideTimer.current = setTimeout(() => setShow(false), 200); }}
    >
      <span className="inline-block w-2 h-2 rounded-full bg-green-500" />
      {show && (
        <div
          className={cn(
            "absolute z-50 left-1/2 -translate-x-1/2 px-4 py-3 bg-white dark:bg-base-100 rounded-xl shadow-2xl border border-gray-200 dark:border-base-300 whitespace-nowrap",
            dropDown ? "top-full mt-1.5" : "bottom-full mb-1.5",
          )}
          onMouseEnter={() => { clearTimeout(hideTimer.current); }}
          onMouseLeave={() => { hideTimer.current = setTimeout(() => setShow(false), 200); }}
        >
          <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
            <span className="text-gray-500 dark:text-gray-400">IP</span>
            <span className="text-gray-900 dark:text-gray-200">{proxy.lastIp || '\u2014'}</span>
            <span className="text-gray-500 dark:text-gray-400">{t('accounts.details.proxy_country', 'Country')}</span>
            <span className="text-gray-900 dark:text-gray-200">{proxy.country || '\u2014'}</span>
            <span className="text-gray-500 dark:text-gray-400">ISP</span>
            <span className="text-gray-900 dark:text-gray-200">{proxy.isp || '\u2014'}</span>
            {(proxy.region || proxy.city) && (
              <>
                <span className="text-gray-500 dark:text-gray-400">{t('accounts.details.proxy_region', 'Region')}</span>
                <span className="text-gray-900 dark:text-gray-200">{[proxy.region, proxy.city].filter(Boolean).join(' \u00b7 ')}</span>
              </>
            )}
            <span className="text-gray-500 dark:text-gray-400">Session</span>
            <span className="text-gray-900 dark:text-gray-200 ">{proxy.sessionId}</span>
            <span className="text-gray-500 dark:text-gray-400">{t('accounts.details.quality', 'Quality')}</span>
            <span className="text-gray-900 dark:text-gray-200">{proxy.quality || '\u2014'}</span>
            {proxy.lastChecked && (
              <>
                <span className="text-gray-500 dark:text-gray-400">{t('accounts.details.last_checked', 'Last Checked')}</span>
                <span className="text-gray-900 dark:text-gray-200 ">{new Date(proxy.lastChecked).toLocaleString()}</span>
              </>
            )}
          </div>
          <div className={cn(
            "absolute left-1/2 -translate-x-1/2 border-4 border-transparent",
            dropDown ? "bottom-full border-b-white dark:border-b-base-100" : "top-full border-t-white dark:border-t-base-100",
          )} />
        </div>
      )}
    </span>
  );
}

// ─── ActionBtn ──────────────────────────────────────────────

function ActionBtn({ icon: Icon, title, onClick, hoverColor, active, spinning, urgent }: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  onClick: () => void;
  hoverColor: string;
  /** Blue breathing effect (task running) */
  active?: boolean;
  /** Spinning effect (refresh etc.) */
  spinning?: boolean;
  /** Persistent red (needs user attention) */
  urgent?: boolean;
}) {
  return (
    <button
      className={cn(
        'p-1 rounded-md transition-all',
        urgent
          ? 'text-red-500 dark:text-red-400'
          : active
            ? 'text-blue-400 dark:text-blue-400 animate-[breathe_2s_ease-in-out_infinite]'
            : 'text-gray-500 dark:text-gray-400',
        !active && !urgent && (hoverColors[hoverColor] || hoverColors.sky),
        urgent && 'hover:text-red-600 dark:hover:text-red-300 hover:bg-red-50 dark:hover:bg-red-900/30',
      )}
      onClick={(e) => { e.stopPropagation(); onClick(); }}
      title={title}
    >
      <Icon className={cn("w-3.5 h-3.5", spinning && "animate-spin")} />
    </button>
  );
}

export default AccountTable;
