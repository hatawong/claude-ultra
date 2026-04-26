/**
 * AddAccountDialog — Add account dialog.
 *
 * Simplified flow: create shell Account -> start Web Login -> on success
 * write cookies -> account appears in list. Supports Google Auth / Email /
 * any login method (user operates in browser).
 *
 * Static proxy entry: Header Static Badge opens a child dialog that hosts
 * StaticProxyEditPanel (mode='add'). Test calls dryrun IPC (no disk write);
 * Save captures sp + ProxySection snapshot in parent state and forwards
 * both to add_account_and_login at submit time for atomic write.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { X, Play, Square, Pause, Trash2, Plus } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import type { StaticProxy } from '../../types/account';
import StaticProxyEditPanel from './StaticProxyEditPanel';
import type { ProxyTestResult } from '../../types/account';
import { cn } from '../../utils/cn';
import { useTaskDialog } from '../../hooks/useTaskDialog';
import { useWebappPreflight } from '../../hooks/useWebappPreflight';

interface AddAccountDialogProps {
  accountId: string | null;
  onAccountIdChange: (id: string | null) => void;
  staticProxy: StaticProxy | null;
  onStaticProxyChange: (sp: StaticProxy | null) => void;
  testResult: ProxyTestResult | null;
  onTestResultChange: (tr: ProxyTestResult | null) => void;
  onClose: () => void;
  onToast?: (msg: string) => void;
}

interface AddAccountResult {
  accountId: string;
  taskId: string;
}

export default function AddAccountDialog({
  accountId, onAccountIdChange, staticProxy, onStaticProxyChange,
  testResult, onTestResultChange, onClose, onToast,
}: AddAccountDialogProps) {
  const { t } = useTranslation();
  const setAccountId = onAccountIdChange;
  const taskId = 'web-add';

  const task = useTaskDialog({ taskId });

  const [showStaticEditDialog, setShowStaticEditDialog] = useState(false);
  const badgeDisabled = task.status === 'running' || task.status === 'paused';

  const toastShownRef = useRef(false);
  useEffect(() => {
    if (task.status === 'done' && !toastShownRef.current) {
      toastShownRef.current = true;
      onToast?.(t('accounts.add.success', 'Account added successfully'));
    }
    if (task.status === 'idle' || task.status === 'running') {
      toastShownRef.current = false;
    }
  }, [task.status, onToast, t]);

  const showStartBtn = !accountId && task.status !== 'running' && task.status !== 'paused';
  const preflight = useWebappPreflight(showStartBtn);

  const handleStart = useCallback(async () => {
    const probedProxy = testResult?.ok ? testResult.proxySection ?? null : null;
    const routeMode = staticProxy ? 'static' : undefined;
    if (accountId) {
      await task.resetForNewRun();
      try {
        await invoke<AddAccountResult>('add_account_and_login', {
          accountId, routeMode, staticProxy, probedProxy,
        });
      } catch (e: any) {
        task.setError(String(e));
      }
      return;
    }
    try {
      const result = await invoke<AddAccountResult>('add_account_and_login', {
        routeMode, staticProxy, probedProxy,
      });
      setAccountId(result.accountId);
    } catch (e: any) {
      task.setError(String(e));
    }
  }, [accountId, staticProxy, testResult]);

  const handleAddAnother = useCallback(() => {
    setAccountId(null);
    onStaticProxyChange(null);
    onTestResultChange(null);
    // Reset progress bar / log / status icon so the next attempt does not
    // start with stale "✅ Done 100%" UI from the previous run.
    task.resetToIdle();
  }, [onStaticProxyChange, onTestResultChange, task]);

  const statusIcon = task.status === 'paused' ? '\u23f8' : {
    idle: '', running: '\ud83d\udfe2', done: '\u2705', failed: '\u274c', aborted: '\u26d4',
  }[task.status];

  const statusText = task.status === 'paused' ? t('task.paused', 'Paused') : {
    idle: '', running: t('task.running', 'Running...'), done: t('task.done', 'Done'), failed: t('task.failed', 'Failed'), aborted: t('task.aborted', 'Aborted'),
  }[task.status];

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={onClose}>
      <div
        className="bg-base-100 rounded-2xl shadow-2xl w-[720px] h-[450px] flex flex-col border border-base-300"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 pt-5 pb-3">
          <div>
            <div className="text-sm font-semibold text-base-content">
              {t('accounts.add.title', 'Add Account')}
            </div>
            <div className="flex items-center gap-2 mt-1">
              <span className="text-xs text-gray-400">
                {t('accounts.add.web_login_hint', 'Login with any method (Google, Email, etc.) in the browser window')}
              </span>
              <button
                type="button"
                disabled={badgeDisabled}
                onClick={() => setShowStaticEditDialog(true)}
                title={
                  badgeDisabled
                    ? t('accounts.add.static_badge_disabled', 'Cannot edit while task is running')
                    : staticProxy
                      ? t('accounts.add.static_badge_set', 'Static proxy configured (click to edit)')
                      : t('accounts.add.static_badge_unset', 'Click to configure static proxy')
                }
                className={cn(
                  'inline-flex items-center px-2 py-0.5 rounded text-[10px] font-semibold transition-colors',
                  staticProxy
                    ? 'bg-green-500/20 text-green-400'
                    : 'bg-gray-500/20 text-gray-400',
                  badgeDisabled
                    ? 'cursor-not-allowed opacity-50'
                    : staticProxy
                      ? 'cursor-pointer hover:bg-green-500/40 hover:text-green-300'
                      : 'cursor-pointer hover:bg-gray-500/40 hover:text-gray-200',
                )}
              >
                {staticProxy ? `Static/${staticProxy.protocol.toUpperCase()}` : 'Static'}
              </button>
            </div>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-md text-gray-400 hover:text-gray-300 hover:bg-base-200 transition-colors">
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Progress */}
        {task.totalSteps > 0 && (
          <div className="px-6 py-3 border-t border-base-200">
            <div className="flex items-center gap-3 mb-2">
              <span className="text-xs text-gray-400">Step {task.step}/{task.totalSteps} · {task.stepName}</span>
              <div className="flex-1 h-2 bg-base-300 rounded-full overflow-hidden">
                <div
                  className={cn(
                    'h-full rounded-full transition-all duration-500',
                    task.status === 'failed' ? 'bg-red-500' : task.status === 'done' ? 'bg-green-500'
                      : task.status === 'paused' ? 'bg-yellow-500' : 'bg-blue-500',
                  )}
                  style={{ width: `${task.percent}%` }}
                />
              </div>
              <span className="text-xs text-gray-500 w-10 text-right">{task.percent}%</span>
            </div>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span className={cn("text-xs", task.status === 'paused' && "text-yellow-400")}>{statusIcon}</span>
                <span className="text-xs text-gray-400">{statusText}</span>
                {task.error && <span className="text-xs text-red-500 max-w-[400px] whitespace-pre-line" title={task.error}>: {task.error}</span>}
              </div>
              <div className="flex items-center gap-1.5">
                {(task.status === 'running' || task.status === 'paused') && (
                  <>
                    <button onClick={task.handlePauseResume} className={cn("px-2.5 py-1 text-xs font-medium rounded-lg transition-colors flex items-center gap-1", task.status === 'paused' ? "bg-green-900/20 text-green-400 hover:bg-green-900/30" : "bg-yellow-900/20 text-yellow-400 hover:bg-yellow-900/30")}>
                      {task.status === 'paused' ? <Play className="w-3 h-3" /> : <Pause className="w-3 h-3" />}
                      {task.status === 'paused' ? t('task.resume', 'Resume') : t('task.pause', 'Pause')}
                    </button>
                    <button onClick={task.handleAbort} className="px-2.5 py-1 text-xs font-medium bg-red-900/20 text-red-400 rounded-lg hover:bg-red-900/30 transition-colors flex items-center gap-1">
                      <Square className="w-3 h-3" />{t('task.abort', 'Abort')}
                    </button>
                  </>
                )}
              </div>
            </div>
          </div>
        )}

        {/* Log area */}
        <div className="flex-1 min-h-0 px-6">
          <div className="bg-gray-900 rounded-lg p-3 h-full overflow-y-auto font-mono text-xs text-gray-300 leading-relaxed">
            {task.logContent ? (
              task.logContent.split('\n').map((line, i) => (
                <div key={i} className={line.includes('\u274c') || line.includes('失败') || line.includes('failed') || line.includes('Error') || line.includes('error:') ? 'text-red-400' : line.includes('\u2705') ? 'text-green-400' : ''}>
                  {line}
                </div>
              ))
            ) : (
              <span className="text-gray-500">
                {!accountId ? t('accounts.add.click_to_start', 'Click "Add Account" to begin') : t('task.waitingLog', 'Waiting for logs...')}
              </span>
            )}
            {preflight.issues.length > 0 && preflight.issues.map((issue, i) => (
              <div key={`pf-${i}`} className="text-red-400">❌ {issue}</div>
            ))}
            <div ref={task.logEndRef} />
          </div>
        </div>

        {/* Footer */}
        <div className="px-6 py-3 border-t border-base-200 flex items-center justify-between">
          <div className="flex items-center gap-2">
            {showStartBtn && (
              <button onClick={handleStart} disabled={!preflight.ready} className={cn("px-4 py-1.5 text-xs font-medium rounded-lg transition-colors flex items-center gap-1", preflight.ready ? "bg-blue-500 text-white hover:bg-blue-600" : "bg-gray-600 text-gray-400 cursor-not-allowed")}>
                <Plus className="w-3 h-3" />{t('accounts.add.start_btn', 'Add Account')}
              </button>
            )}
            {accountId && (task.status === 'failed' || task.status === 'aborted') && (
              <button onClick={handleStart} className="px-4 py-1.5 text-xs font-medium bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1">
                <Plus className="w-3 h-3" />{t('accounts.add.start_btn', 'Add Account')}
              </button>
            )}
            {accountId && task.status === 'done' && (
              <button onClick={handleAddAnother} className="px-4 py-1.5 text-xs font-medium bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1">
                <Plus className="w-3 h-3" />{t('accounts.add.add_another', 'Add Another')}
              </button>
            )}
            {task.status !== 'running' && task.status !== 'paused' && accountId && (
              <button onClick={task.handleClearLog} className="px-3 py-1.5 text-xs font-medium bg-gray-700/30 text-gray-400 rounded-lg hover:bg-gray-700/50 transition-colors flex items-center gap-1">
                <Trash2 className="w-3 h-3" />{t('task.clearLog', 'Clear Log')}
              </button>
            )}
          </div>
          <div className="flex items-center gap-2">
            {task.toast && <span className="text-xs text-green-400">{task.toast}</span>}
            <button onClick={onClose} className="px-4 py-1.5 text-xs font-medium bg-base-200 text-gray-300 rounded-lg hover:bg-base-300 transition-colors">
              {t('common.close', 'Close')}
            </button>
          </div>
        </div>
      </div>

      {/* Child dialog: Static Proxy Edit (z-[55] above AddDialog z-50) */}
      {showStaticEditDialog && (
        <div
          className="fixed inset-0 z-[55] flex items-center justify-center bg-black/50"
          onClick={(e) => {
            // Stop bubbling so closing the child dialog does not also close
            // the parent AddAccountDialog (parent backdrop has onClick=onClose).
            e.stopPropagation();
            setShowStaticEditDialog(false);
          }}
        >
          <div
            className="bg-base-100 rounded-xl shadow-2xl w-[420px] p-4 border border-base-300"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-sm font-semibold text-base-content">
                {t('accounts.add.static_edit_title', 'Configure Static Proxy')}
              </h3>
              <button
                onClick={() => setShowStaticEditDialog(false)}
                className="p-1 rounded text-gray-400 hover:text-gray-300"
              >
                <X className="w-4 h-4" />
              </button>
            </div>
            <StaticProxyEditPanel
              mode="add"
              initialStaticProxy={staticProxy}
              initialTestResult={testResult}
              onSave={(sp, tr) => {
                onStaticProxyChange(sp);
                onTestResultChange(tr);
                setShowStaticEditDialog(false);
              }}
              onDelete={() => {
                onStaticProxyChange(null);
                onTestResultChange(null);
                setShowStaticEditDialog(false);
              }}
              onToast={(msg) => onToast?.(msg)}
            />
          </div>
        </div>
      )}
    </div>
  );
}
