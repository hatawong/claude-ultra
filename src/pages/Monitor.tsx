/**
 * Monitor — Gateway request log viewer
 * Aligned with AM ProxyMonitor UI:
 * - Table in card container (matching Accounts)
 * - Quick filters + account dropdown + logging toggle
 * - Detail modal with request/response body
 * - Debounced event updates + confirm dialog
 * - Full i18n
 */
import { useEffect, useState, useRef, useMemo, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import {
  Trash2, Search, X, RefreshCw, Copy, Check, Loader2,
} from 'lucide-react';
import * as gatewayService from '../services/gatewayService';
import type { RequestLog } from '../services/gatewayService';
import { cn } from '../utils/cn';
import Pagination from '../components/common/Pagination';
import ConfirmDialog from '../components/common/ConfirmDialog';
import JsonHighlight from '../components/common/JsonHighlight';

// ── Formatters ─────────────────────────────────────────────

const formatTokens = (n: number | null): string => {
  if (n == null) return '\u2014';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toString();
};

const formatDuration = (ms: number): string => {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${ms}ms`;
};

const statusColor = (status: number): string => {
  if (status >= 200 && status < 300) return 'bg-green-500';
  if (status === 429) return 'bg-amber-500';
  if (status === 401 || status === 403) return 'bg-red-500';
  if (status >= 500) return 'bg-red-600';
  return 'bg-gray-400';
};

const formatCost = (cost: number | null): string => {
  if (cost == null || cost === 0) return '\u2014';
  if (cost >= 1) return `$${cost.toFixed(2)}`;
  if (cost >= 0.01) return `$${cost.toFixed(3)}`;
  return `$${cost.toFixed(6)}`;
};

function formatBody(body: string | null): string {
  if (!body) return '';
  try { return JSON.stringify(JSON.parse(body), null, 2); }
  catch { return body; }
}


type StatusFilter = 'all' | 'success' | 'error';

// ── Constants ──────────────────────────────────────────────

const PAGE_SIZE_OPTIONS = [50, 100, 200, 500];

// ── Main Monitor Component ─────────────────────────────────

function Monitor() {
  const { t } = useTranslation();
  const [logs, setLogs] = useState<RequestLog[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(50);
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(false);
  const [selectedLog, setSelectedLog] = useState<RequestLog | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  // Filters
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [accountFilter, setAccountFilter] = useState('');

  // Logging toggle
  const [loggingEnabled, setLoggingEnabled] = useState(false);

  // Clear confirm
  const [showClearConfirm, setShowClearConfirm] = useState(false);

  // Copy state
  const [copied, setCopied] = useState<string | null>(null);

  const searchRef = useRef(search);
  const pageRef = useRef(currentPage);
  const loadDataRef = useRef<(page?: number, searchText?: string) => void>(() => {});
  const isMountedRef = useRef(true);
  const loadingRef = useRef(false);

  // Debounce timer for signal-based refresh
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const loadData = useCallback(async (page = 1, searchText = search) => {
    if (loadingRef.current) return;
    loadingRef.current = true;
    setLoading(true);
    try {
      const timeoutPromise = new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error('Request timeout')), 10000),
      );
      const [count, data] = await Promise.race([
        Promise.all([
          gatewayService.getLogsCount(searchText || undefined),
          gatewayService.getRequestLogs(
            pageSize, (page - 1) * pageSize,
            undefined, undefined, undefined, undefined,
            searchText || undefined,
          ),
        ]),
        timeoutPromise,
      ]) as [number, RequestLog[]];
      if (isMountedRef.current) {
        setTotalCount(count);
        setLogs(data);
      }
    } catch (e) {
      console.error('Failed to load monitor data:', e);
    } finally {
      loadingRef.current = false;
      if (isMountedRef.current) setLoading(false);
    }
  }, [pageSize, search]);

  // Keep ref in sync with latest loadData
  loadDataRef.current = loadData;

  // Load logging state on mount
  useEffect(() => {
    (async () => {
      try {
        const info = await invoke<{ enableLogging?: boolean }>('get_gateway_connection_info');
        setLoggingEnabled(info.enableLogging ?? false);
      } catch {}
    })();
  }, []);

  // Initial load + signal-based refresh (single data source: DB)
  useEffect(() => {
    isMountedRef.current = true;
    loadData(1, '');

    let unlistenFn: (() => void) | null = null;

    const setup = async () => {
      // Listen for log-updated signal — backend emits when a log has usage or error
      unlistenFn = await listen<void>('gateway://log-updated', () => {
        if (!isMountedRef.current) return;
        if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
        refreshTimerRef.current = setTimeout(() => {
          loadDataRef.current(pageRef.current, searchRef.current);
        }, 500);
      });
    };
    setup();

    return () => {
      isMountedRef.current = false;
      if (unlistenFn) unlistenFn();
      if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
    };
  }, []);

  // Reload when pageSize changes
  useEffect(() => {
    setCurrentPage(1);
    pageRef.current = 1;
    loadData(1, search);
  }, [pageSize]);

  // Reload when search changes (debounced)
  useEffect(() => {
    searchRef.current = search;
    const timer = setTimeout(() => {
      setCurrentPage(1);
      pageRef.current = 1;
      loadData(1, search);
    }, 300);
    return () => clearTimeout(timer);
  }, [search]);

  const goToPage = (page: number) => {
    const tp = Math.max(1, Math.ceil(totalCount / pageSize));
    if (page < 1 || page > tp || page === currentPage) return;
    setCurrentPage(page);
    pageRef.current = page;
    loadData(page, search);
  };

  const handleClear = async () => {
    try {
      await gatewayService.clearGatewayLogs();
      setLogs([]);
      setTotalCount(0);
    } catch (e) {
      console.error('Failed to clear logs:', e);
    }
    setShowClearConfirm(false);
  };

  const handleToggleLogging = async () => {
    const next = !loggingEnabled;
    try {
      await gatewayService.setLoggingEnabled(next);
      setLoggingEnabled(next);
    } catch (e) {
      console.error('Failed to toggle logging:', e);
    }
  };

  const detailRequestIdRef = useRef(0);
  const handleRowClick = async (log: RequestLog) => {
    const requestId = ++detailRequestIdRef.current;
    setSelectedLog(log);
    setDetailLoading(true);
    try {
      const detail = await gatewayService.getLogDetail(log.id);
      // Only update if this is still the latest request and modal is still open
      if (detailRequestIdRef.current === requestId) {
        setSelectedLog(detail);
      }
    } catch {
      // Keep the basic log if detail fetch fails
    } finally {
      if (detailRequestIdRef.current === requestId) {
        setDetailLoading(false);
      }
    }
  };

  const copyToClipboard = async (text: string, label: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(label);
    setTimeout(() => setCopied(null), 2000);
  };

  // Derived: unique accounts for filter dropdown
  const accountEmails = useMemo(() => {
    const emails = new Set<string>();
    logs.forEach(l => { if (l.account_email) emails.add(l.account_email); });
    return Array.from(emails).sort();
  }, [logs]);

  // Filtered logs (status + account, applied on frontend)
  const filteredLogs = useMemo(() => {
    let result = logs;
    if (statusFilter === 'success') {
      result = result.filter(l => l.status >= 200 && l.status < 400);
    } else if (statusFilter === 'error') {
      result = result.filter(l => l.status >= 400);
    }
    if (accountFilter) {
      result = result.filter(l => l.account_email === accountFilter);
    }
    return result;
  }, [logs, statusFilter, accountFilter]);

  const pageStats = useMemo(() => {
    const success = logs.filter(l => l.status >= 200 && l.status < 400).length;
    const error = logs.filter(l => l.status >= 400).length;
    return { success, error };
  }, [logs]);

  const totalPages = Math.max(1, Math.ceil(totalCount / pageSize));

  return (
    <div className="h-full w-full flex flex-col overflow-hidden p-5 gap-4">
      {/* Toolbar */}
      <div className="space-y-2 shrink-0">
        {/* Row 1: Search + stats + actions */}
        <div className="flex items-center gap-3">
          {/* Logging toggle */}
          <button
            onClick={handleToggleLogging}
            className={cn(
              'px-3 py-1.5 rounded-lg text-xs font-medium flex items-center gap-2 transition-colors border shrink-0',
              loggingEnabled
                ? 'bg-red-50 dark:bg-red-900/20 text-red-600 dark:text-red-400 border-red-200 dark:border-red-800'
                : 'bg-gray-100 dark:bg-base-200 text-gray-500 dark:text-gray-400 border-gray-200 dark:border-base-300',
            )}
          >
            <span className={cn(
              'w-2 h-2 rounded-full',
              loggingEnabled ? 'bg-red-500 animate-pulse' : 'bg-gray-400',
            )} />
            {loggingEnabled ? t('monitor.toolbar.recording', 'Recording') : t('monitor.toolbar.stopped', 'Stopped')}
          </button>

          {/* Search */}
          <div className="relative flex-1">
            <Search className="absolute left-2.5 top-2 text-gray-400" size={14} />
            <input
              type="text"
              placeholder={t('monitor.toolbar.search_placeholder', 'Search model, account, error...')}
              className="w-full pl-9 pr-3 py-1.5 text-xs border border-gray-200 dark:border-gray-600 rounded-lg bg-white dark:bg-base-200 text-gray-900 dark:text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
              value={search}
              onChange={e => setSearch(e.target.value)}
            />
          </div>

          {/* Account filter */}
          <select
            value={accountFilter}
            onChange={e => setAccountFilter(e.target.value)}
            className="px-2 py-1.5 text-xs border border-gray-200 dark:border-base-300 rounded-lg bg-white dark:bg-base-200 text-gray-900 dark:text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500 max-w-[180px]"
          >
            <option value="">{t('monitor.toolbar.all_accounts', 'All Accounts')}</option>
            {accountEmails.map(email => (
              <option key={email} value={email}>{email}</option>
            ))}
          </select>

          {/* Stats */}
          <div className="hidden lg:flex gap-3 text-[10px] font-bold uppercase shrink-0">
            <span className="text-blue-500">{totalCount} {t('monitor.toolbar.total', 'Total')}</span>
            <span className="text-green-500">{pageStats.success} {t('monitor.toolbar.ok', 'OK')}</span>
            <span className="text-red-500">{pageStats.error} {t('monitor.toolbar.err', 'ERR')}</span>
          </div>

          <button onClick={() => loadData(currentPage, search)} className="p-1.5 rounded-md text-gray-400 hover:text-gray-600 dark:hover:text-white transition-colors" title={t('common.refresh', 'Refresh')}>
            <RefreshCw size={16} className={loading ? 'animate-spin' : ''} />
          </button>
          <button onClick={() => setShowClearConfirm(true)} className="p-1.5 rounded-md text-gray-400 hover:text-red-500 transition-colors" title={t('monitor.toolbar.clear', 'Clear all logs')}>
            <Trash2 size={16} />
          </button>
        </div>

        {/* Row 2: Quick filters + logging disabled warning */}
        <div className="flex items-center gap-2">
          <div className="flex items-center gap-1">
            <span className="text-[10px] text-gray-400 mr-1">{t('monitor.filter.label', 'Filter')}:</span>
            {(['all', 'success', 'error'] as StatusFilter[]).map(f => (
              <button
                key={f}
                onClick={() => setStatusFilter(f)}
                className={cn(
                  'px-3 py-1 rounded-full text-[11px] font-medium transition-all',
                  statusFilter === f
                    ? 'bg-gray-900 text-white dark:bg-white dark:text-gray-900'
                    : 'text-gray-500 hover:bg-gray-100 dark:hover:bg-base-200',
                )}
              >
                {f === 'all' && t('monitor.filter.all', 'All')}
                {f === 'success' && t('monitor.filter.success', 'Success')}
                {f === 'error' && t('monitor.filter.error', 'Error')}
              </button>
            ))}
          </div>
          <div className="flex-1" />
          {!loggingEnabled && (
            <div className="flex items-center gap-2 text-xs text-amber-600 dark:text-amber-400">
              <span>{t('monitor.banner.logging_disabled', 'Request logging is disabled. New requests will not be recorded.')}</span>
              <button
                onClick={handleToggleLogging}
                className="px-2 py-0.5 text-[11px] font-medium bg-amber-500 hover:bg-amber-600 text-white rounded transition-colors shrink-0"
              >
                {t('monitor.banner.enable', 'Enable')}
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Table in card container */}
      <div className="flex-1 min-h-0 bg-white dark:bg-base-100 rounded-lg shadow-sm border border-gray-100 dark:border-base-200 flex flex-col overflow-hidden">
        <div className="flex-1 overflow-y-auto overflow-x-auto">
          <table className="w-full text-xs">
            <thead className="bg-gray-50 dark:bg-base-200 text-gray-500 dark:text-gray-400 sticky top-0 z-10">
              <tr className="border-b border-gray-100 dark:border-base-200">
                <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 60 }}>{t('monitor.table.status', 'Status')}</th>
                <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 60 }}>{t('monitor.table.method', 'Method')}</th>
                <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 180 }}>{t('monitor.table.model', 'Model')}</th>
                <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 140 }}>{t('monitor.table.account', 'Account')}</th>
                <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 90 }}>{t('monitor.table.input', 'Input')}</th>
                <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 70 }}>{t('monitor.table.output', 'Output')}</th>
                <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 80 }}>{t('monitor.table.cost', 'Cost')}</th>
                <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 80 }}>{t('monitor.table.duration', 'Duration')}</th>
                <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 80 }}>{t('monitor.table.time', 'Time')}</th>
              </tr>
            </thead>
            <tbody className="text-gray-700 dark:text-gray-300 divide-y divide-gray-50 dark:divide-base-200">
              {filteredLogs.map(log => (
                <tr
                  key={log.id}
                  className="hover:bg-blue-50 dark:hover:bg-blue-900/20 cursor-pointer transition-colors"
                  onClick={() => handleRowClick(log)}
                >
                  <td className="py-1.5 px-3">
                    <span className={`px-1.5 py-0.5 rounded text-white text-[10px] font-bold ${statusColor(log.status)}`}>
                      {log.status}
                    </span>
                  </td>
                  <td className="py-1.5 px-3 font-bold">{log.method}</td>
                  <td className="py-1.5 px-3 text-blue-600 dark:text-blue-400 truncate" style={{ maxWidth: 180 }}>
                    {log.model || '\u2014'}
                  </td>
                  <td className="py-1.5 px-3 text-gray-500 truncate text-[10px]" style={{ maxWidth: 200 }}>
                    {log.account_email || '\u2014'}
                  </td>
                  <td className="py-1.5 px-3 text-left">
                    <div className="text-[10px] leading-relaxed">
                      <div>{formatTokens(log.input_tokens)}</div>
                      {(log.cache_read_tokens != null && log.cache_read_tokens > 0) ? (
                        <div className="text-gray-400">{t('monitor.table.cache_read', 'Cache')}: {formatTokens(log.cache_read_tokens)}</div>
                      ) : <div>&nbsp;</div>}
                    </div>
                  </td>
                  <td className="py-1.5 px-3 text-left">
                    <div className="text-[10px] leading-relaxed">
                      <div>{formatTokens(log.output_tokens)}</div>
                      <div>&nbsp;</div>
                    </div>
                  </td>
                  <td className={`py-1.5 px-3 text-right text-[10px] ${(log.total_cost ?? 0) > 5 ? 'text-red-500 font-bold' : 'text-green-600 dark:text-green-400'}`}>{formatCost(log.total_cost)}</td>
                  <td className="py-1.5 px-3 text-right text-[10px]">{formatDuration(log.duration_ms)}</td>
                  <td className="py-1.5 px-3 text-right text-[10px]">
                    {new Date(log.timestamp).toLocaleTimeString('en-US', { hour12: false })}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {loading && (
            <div className="flex items-center justify-center p-4">
              <RefreshCw size={16} className="animate-spin text-gray-400 mr-2" />
              <span className="text-sm text-gray-500">{t('common.loading', 'Loading...')}</span>
            </div>
          )}

          {!loading && filteredLogs.length === 0 && (
            <div className="flex items-center justify-center p-12 text-gray-400 text-sm">
              {t('monitor.empty', 'No request logs yet. Start the gateway and send some requests.')}
            </div>
          )}
        </div>
      </div>

      {/* Pagination — outside card, same as Accounts */}
      <div className="flex-none shrink-0">
        <Pagination
          currentPage={currentPage}
          totalPages={totalPages}
          onPageChange={goToPage}
          totalItems={totalCount}
          itemsPerPage={pageSize}
          totalOnly={true}
          onPageSizeChange={(s) => { setPageSize(s); setCurrentPage(1); pageRef.current = 1; }}
          pageSizeOptions={PAGE_SIZE_OPTIONS}
        />
      </div>

      {/* Detail modal */}
      {selectedLog && (
        <DetailModal
          log={selectedLog}
          loading={detailLoading}
          copied={copied}
          onCopy={copyToClipboard}
          onClose={() => { detailRequestIdRef.current++; setSelectedLog(null); }}
        />
      )}

      {/* Clear confirm dialog */}
      {showClearConfirm && (
        <ConfirmDialog
          title={t('monitor.dialog.clear_title', 'Clear Logs')}
          message={t('monitor.dialog.clear_message', 'Are you sure you want to clear all request logs? This action cannot be undone.')}
          confirmText={t('common.delete', 'Delete')}
          confirmColor="red"
          onConfirm={handleClear}
          onCancel={() => setShowClearConfirm(false)}
        />
      )}
    </div>
  );
}

// ── Detail Modal ───────────────────────────────────────────

function DetailModal({ log, loading, copied, onCopy, onClose }: {
  log: RequestLog;
  loading: boolean;
  copied: string | null;
  onCopy: (text: string, label: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={onClose}>
      <div className="bg-white dark:bg-base-100 rounded-xl shadow-2xl w-full max-w-4xl max-h-[85vh] flex flex-col overflow-hidden border border-gray-200 dark:border-base-300" onClick={e => e.stopPropagation()}>
        {/* Header */}
        <div className="px-4 py-3 border-b border-gray-100 dark:border-base-300 flex items-center justify-between bg-gray-50 dark:bg-base-200">
          <div className="flex items-center gap-3">
            {loading && <Loader2 size={14} className="animate-spin text-gray-400" />}
            <span className={`px-2 py-0.5 rounded text-white text-xs font-bold ${statusColor(log.status)}`}>{log.status}</span>
            <span className="font-bold text-gray-900 dark:text-base-content text-sm">{log.method}</span>
            <span className="text-xs text-gray-500 truncate max-w-md">{log.url}</span>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-md text-gray-400 hover:text-gray-800 dark:hover:text-white hover:bg-gray-200 dark:hover:bg-base-300 transition-colors">
            <X size={18} />
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          {/* Metadata */}
          <div className="grid grid-cols-2 lg:grid-cols-3 gap-4">
            <Field label={t('monitor.details.account', 'Account')} value={log.account_email || '\u2014'} />
            <Field label={t('monitor.details.account_id', 'Account ID')} value={log.account_id || '\u2014'} />
            <Field label={t('monitor.details.model', 'Model')} value={log.model || '\u2014'} className="text-blue-600 dark:text-blue-400 font-bold" />
            <Field label={t('monitor.details.time', 'Time')} value={new Date(log.timestamp).toLocaleString()} />
            <Field label={t('monitor.details.duration', 'Duration')} value={formatDuration(log.duration_ms)} />
            <Field
              label={t('monitor.details.req_resp_size', 'Request / Response Size')}
              value={`${log.request_size != null ? `${(log.request_size / 1024).toFixed(1)} KB` : '\u2014'} / ${log.response_size != null ? `${(log.response_size / 1024).toFixed(1)} KB` : '\u2014'}`}
            />
          </div>

          {/* Token details */}
          <div>
            <h3 className="text-xs font-bold uppercase text-gray-400 mb-3">{t('monitor.details.token_usage', 'Token Usage')}</h3>
            <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
              <TokenBadge label={t('monitor.details.input', 'Input')} value={log.input_tokens} color="blue" />
              <TokenBadge label={t('monitor.details.output', 'Output')} value={log.output_tokens} color="green" />
              <TokenBadge label={t('monitor.details.cache_write', 'Cache Write')} value={log.cache_creation_tokens} color="amber" />
              <TokenBadge label={t('monitor.details.cache_read', 'Cache Read')} value={log.cache_read_tokens} color="cyan" />
            </div>
            <div className="mt-3 pt-3 border-t border-gray-200 dark:border-base-300 flex items-center gap-4">
              <span className="text-sm font-bold text-gray-800 dark:text-white">{t('common.total_label', 'Total')}: {formatTokens(log.total_tokens)}</span>
              <span className="text-sm font-bold text-green-600">{formatCost(log.total_cost)}</span>
            </div>
          </div>

          {/* Cost breakdown */}
          {log.total_cost != null && log.total_cost > 0 && (
            <div>
              <h3 className="text-xs font-bold uppercase text-gray-400 mb-3">{t('monitor.details.cost_breakdown', 'Cost Breakdown')}</h3>
              <div className="grid grid-cols-2 lg:grid-cols-4 gap-3 text-sm">
                <div><span className="text-gray-500">{t('monitor.details.input', 'Input')}:</span> <span>{formatCost(log.input_cost)}</span></div>
                <div><span className="text-gray-500">{t('monitor.details.output', 'Output')}:</span> <span>{formatCost(log.output_cost)}</span></div>
                <div><span className="text-gray-500">{t('monitor.details.cache_write', 'Cache Write')}:</span> <span>{formatCost(log.cache_creation_cost)}</span></div>
                <div><span className="text-gray-500">{t('monitor.details.cache_read', 'Cache Read')}:</span> <span>{formatCost(log.cache_read_cost)}</span></div>
              </div>
            </div>
          )}

          {/* Error */}
          {log.error && (
            <div className="bg-red-50 dark:bg-red-900/20 p-4 rounded-lg border border-red-200 dark:border-red-800">
              <h3 className="text-xs font-bold uppercase text-red-500 mb-2">{t('monitor.details.error', 'Error')}</h3>
              <pre className="text-sm font-mono text-red-700 dark:text-red-300 whitespace-pre-wrap">{log.error}</pre>
            </div>
          )}

          {/* Request Payload */}
          <div>
            <div className="flex items-center justify-between mb-2">
              <h3 className="text-xs font-bold uppercase text-gray-400">{t('monitor.details.request_payload', 'Request Payload')}</h3>
              {log.request_body && (
                <button
                  onClick={() => onCopy(formatBody(log.request_body), 'req')}
                  className="text-xs text-gray-400 hover:text-blue-500 transition-colors flex items-center gap-1"
                >
                  {copied === 'req' ? <Check size={12} className="text-green-500" /> : <Copy size={12} />}
                  {copied === 'req' ? t('common.copied', 'Copied') : t('common.copy', 'Copy')}
                </button>
              )}
            </div>
            {loading ? (
              <div className="flex items-center justify-center py-4">
                <Loader2 size={16} className="animate-spin text-gray-400" />
              </div>
            ) : log.request_body ? (
              <pre className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-gray-100 dark:border-base-200 text-[11px] text-gray-700 dark:text-gray-300 overflow-x-auto max-h-60 overflow-y-auto whitespace-pre-wrap">
                <JsonHighlight json={formatBody(log.request_body)} />
              </pre>
            ) : (
              <p className="text-xs text-gray-400 italic">{t('monitor.details.no_data', 'No data')}</p>
            )}
          </div>

          {/* Response Payload */}
          <div>
            <div className="flex items-center justify-between mb-2">
              <h3 className="text-xs font-bold uppercase text-gray-400">{t('monitor.details.response_payload', 'Response Payload')}</h3>
              {log.response_body && (
                <button
                  onClick={() => onCopy(formatBody(log.response_body), 'resp')}
                  className="text-xs text-gray-400 hover:text-blue-500 transition-colors flex items-center gap-1"
                >
                  {copied === 'resp' ? <Check size={12} className="text-green-500" /> : <Copy size={12} />}
                  {copied === 'resp' ? t('common.copied', 'Copied') : t('common.copy', 'Copy')}
                </button>
              )}
            </div>
            {loading ? (
              <div className="flex items-center justify-center py-4">
                <Loader2 size={16} className="animate-spin text-gray-400" />
              </div>
            ) : log.response_body ? (
              <pre className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-gray-100 dark:border-base-200 text-[11px] text-gray-700 dark:text-gray-300 overflow-x-auto max-h-60 overflow-y-auto whitespace-pre-wrap">
                <JsonHighlight json={formatBody(log.response_body)} />
              </pre>
            ) : (
              <p className="text-xs text-gray-400 italic">{t('monitor.details.no_data', 'No data')}</p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Field({ label, value, className }: { label: string; value: string; className?: string }) {
  return (
    <div>
      <span className="block text-gray-400 uppercase font-bold text-[10px] tracking-widest mb-1">{label}</span>
      <span className={cn('text-xs text-gray-900 dark:text-white', className)}>{value}</span>
    </div>
  );
}

function TokenBadge({ label, value, color }: { label: string; value: number | null; color: string }) {
  const colorMap: Record<string, string> = {
    blue: 'text-blue-700 dark:text-blue-300 bg-blue-100 dark:bg-blue-900/40 border-blue-200 dark:border-blue-800/50',
    green: 'text-green-700 dark:text-green-300 bg-green-100 dark:bg-green-900/40 border-green-200 dark:border-green-800/50',
    amber: 'text-amber-700 dark:text-amber-300 bg-amber-100 dark:bg-amber-900/40 border-amber-200 dark:border-amber-800/50',
    cyan: 'text-cyan-700 dark:text-cyan-300 bg-cyan-100 dark:bg-cyan-900/40 border-cyan-200 dark:border-cyan-800/50',
  };
  return (
    <div className={`px-3 py-1.5 rounded-lg border text-[11px] font-bold ${colorMap[color]}`}>
      {label}: {formatTokens(value)}
    </div>
  );
}

export default Monitor;
