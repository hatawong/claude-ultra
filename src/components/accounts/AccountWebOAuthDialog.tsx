/**
 * AccountWebOAuthDialog — OAuth authorization dialog
 * Launches webapp OAuth subprocess to obtain CLI accessToken/refreshToken.
 * Rust handle_oauth_result writes Account.cli automatically; frontend only displays progress.
 */
import { useCallback } from 'react';
import { X, Play, Square, Pause, Trash2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { cn } from '../../utils/cn';
import { useWebappPreflight } from '../../hooks/useWebappPreflight';
import { useTaskDialog } from '../../hooks/useTaskDialog';
import type { Account } from '../../types/account';

interface AccountWebOAuthDialogProps {
  account: Account;
  onClose: () => void;
  onDone: () => void;
}

export default function AccountWebOAuthDialog({ account, onClose, onDone }: AccountWebOAuthDialogProps) {
  const { t } = useTranslation();
  const taskId = `oauth-${account.accountId}`;

  const task = useTaskDialog({
    taskId,
    onEvent: (type, data) => {
      if (type === 'meta') {
        const meta = data?.data || data;
        invoke('update_account_profile', {
          accountId: account.accountId,
          email: meta?.email || null,
          accountUuid: meta?.accountUuid || null,
          orgId: meta?.orgId || null,
          fullName: meta?.fullName || null,
          subscriptionType: meta?.subscriptionType || null,
          rateLimitTier: meta?.rateLimitTier || null,
          billingType: meta?.billingType || null,
        }).then(() => onDone()).catch(() => {});
      }
      if (type === 'cookies') {
        invoke('update_web_login', {
          accountId: account.accountId,
          cookies: data.cookies || [],
          sessionKey: data.sessionKey || '',
          proxy: null,
        }).catch(() => {});
      }
      if (type === 'result' && data.success) {
        // CLI token is written by Rust handle_oauth_result; here we only save cookies + proxy + profile
        invoke('update_web_login', {
          accountId: account.accountId,
          cookies: data.data?.cookies || [],
          sessionKey: data.data?.sessionKey || '',
          proxy: data.data?.proxy || null,
        }).catch(() => {});
        invoke('update_account_profile', {
          accountId: account.accountId,
          email: data.data?.email || null,
          accountUuid: data.data?.accountUuid || null,
          orgId: data.data?.orgId || null,
          fullName: data.data?.fullName || null,
          subscriptionType: data.data?.subscriptionType || null,
          rateLimitTier: data.data?.rateLimitTier || null,
          billingType: data.data?.billingType || null,
        }).catch(() => {});
      }
    },
    onDone,
  });

  const preflight = useWebappPreflight(task.status !== 'running' && task.status !== 'paused');

  const handleStart = useCallback(async () => {
    await task.resetForNewRun();
    try {
      await invoke<string>('start_oauth', { accountId: account.accountId });
    } catch (e: any) {
      task.setError(String(e));
    }
  }, [account.accountId, task.resetForNewRun]);

  const statusIcon = task.status === 'paused' ? '\u23f8' : {
    idle: '', running: '\ud83d\udfe2', done: '\u2705', failed: '\u274c',
  }[task.status];

  const statusText = task.status === 'paused' ? t('task.paused', 'Paused') : {
    idle: t('task.idle', 'Not started'),
    running: t('task.running', 'Running...'),
    done: t('task.done', 'Done'),
    failed: t('task.failed', 'Failed'),
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
            <div className="text-sm font-semibold text-base-content">{t('task.webOAuth', 'Web Authorization')}</div>
            <div className="flex items-center gap-1.5 mt-1">
              {account.email && <span className="text-xs text-gray-400">{account.email}</span>}
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
                <span className="text-xs">{statusIcon}</span>
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
                {task.status === 'idle' ? t('task.clickToStart', 'Click to begin') : t('task.waitingLog', 'Waiting for logs...')}
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
            {task.status === 'idle' && (
              <button onClick={handleStart} disabled={!preflight.ready} className={cn("px-4 py-1.5 text-xs font-medium rounded-lg transition-colors flex items-center gap-1", preflight.ready ? "bg-blue-500 text-white hover:bg-blue-600" : "bg-gray-600 text-gray-400 cursor-not-allowed")}>
                <Play className="w-3 h-3" />{t('task.startOAuth', 'Start OAuth')}
              </button>
            )}
            {(task.status === 'done' || task.status === 'failed') && (
              <button onClick={handleStart} disabled={!preflight.ready} className={cn("px-4 py-1.5 text-xs font-medium rounded-lg transition-colors flex items-center gap-1", preflight.ready ? "bg-blue-500 text-white hover:bg-blue-600" : "bg-gray-600 text-gray-400 cursor-not-allowed")}>
                <Play className="w-3 h-3" />{t('task.retryOAuth', 'Retry OAuth')}
              </button>
            )}
            {task.status !== 'running' && task.status !== 'paused' && (
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
    </div>
  );
}
