/**
 * AddAccountDialog — Add account dialog
 *
 * Simplified flow: create shell Account -> start Web Login -> on success write cookies -> account appears in list
 * Supports Google Auth / Email or any login method (user operates in browser).
 */
import { useCallback, useEffect, useRef } from 'react';
import { X, Play, Square, Pause, Trash2, Plus } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { cn } from '../../utils/cn';
import { useTaskDialog } from '../../hooks/useTaskDialog';
import { useWebappPreflight } from '../../hooks/useWebappPreflight';


interface AddAccountDialogProps {
  accountId: string | null;
  onAccountIdChange: (id: string | null) => void;
  onClose: () => void;
  onToast?: (msg: string) => void;
}

interface AddAccountResult {
  accountId: string;
  taskId: string;
}

export default function AddAccountDialog({ accountId, onAccountIdChange, onClose, onToast }: AddAccountDialogProps) {
  const { t } = useTranslation();
  const setAccountId = onAccountIdChange;
  // web-add is a singleton task (one AddAccount operation globally at a time).
  const taskId = 'web-add';

  // Writes handled by Rust subprocess.rs directly to AccountManager.
  // Dialog is display-only; toast shown on task.status === 'done'.
  const task = useTaskDialog({ taskId });

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
    // On retry, reuse existing accountId; both paths use add_account_and_login → web-add- taskId
    if (accountId) {
      await task.resetForNewRun();
      try {
        await invoke<AddAccountResult>('add_account_and_login', { accountId });
      } catch (e: any) {
        task.setError(String(e));
      }
      return;
    }
    try {
      const result = await invoke<AddAccountResult>('add_account_and_login');
      setAccountId(result.accountId);
    } catch (e: any) {
      task.setError(String(e));
    }
  }, [accountId]);

  const handleAddAnother = useCallback(() => {
    setAccountId(null);
    // Don't call resetForNewRun (it sets running); next button click takes the first-time path in handleStart
  }, []);

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
            <div className="text-sm font-semibold text-base-content">{t('accounts.add.title', 'Add Account')}</div>
            <div className="text-xs text-gray-400 mt-1">
              {t('accounts.add.web_login_hint', 'Login with any method (Google, Email, etc.) in the browser window.')}
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
                {(task.status === 'running' || task.status === 'paused') && !task.hasResult && (
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
            {task.status === 'done' && (
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
    </div>
  );
}
