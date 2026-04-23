/**
 * useTaskDialog — subprocess task dialog logic.
 *
 * Single source of truth: Rust SubprocessTask (queried via get_task IPC +
 * subscribed via subprocess://task event). No log parsing heuristics — status
 * comes from Rust state directly.
 *
 * Log file content is still polled for display in the log panel (separate from state).
 */
import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';

export type TaskStatus = 'idle' | 'running' | 'paused' | 'done' | 'failed' | 'aborted';

export interface TaskDialogState {
  status: TaskStatus;
  step: number;
  totalSteps: number;
  stepName: string;
  percent: number;
  error: string | null;
  /** True when Rust has received Result but wait() hasn't exited yet (webapp in 30s cleanup wait). */
  hasResult: boolean;
  logContent: string;
  toast: string;
  logEndRef: React.RefObject<HTMLDivElement>;
  handlePauseResume: () => Promise<void>;
  handleAbort: () => Promise<void>;
  handleClearLog: () => Promise<void>;
  resetForNewRun: () => Promise<void>;
  setError: (msg: string | null) => void;
}

interface IpcNames {
  getLog: string;
  clearLog: string;
  abort: string;
  pause: string;
  resume: string;
}

const DEFAULT_IPC: IpcNames = {
  getLog: 'get_subprocess_log',
  clearLog: 'clear_subprocess_log',
  abort: 'abort_subprocess',
  pause: 'pause_subprocess',
  resume: 'resume_subprocess',
};

interface UseTaskDialogOptions {
  taskId: string;
  ipc?: Partial<IpcNames>;
}

interface TaskSnapshot {
  taskId: string;
  accountId: string;
  subprocessType: string;
  flow: string;
  status: 'running' | 'paused' | 'done' | 'failed' | 'aborted';
  step: number;
  totalSteps: number;
  stepName: string;
  error: string | null;
  result: any | null;
  startedAt: number;
  finishedAt: number | null;
  lastEventAt: number;
}

export function useTaskDialog({ taskId, ipc }: UseTaskDialogOptions): TaskDialogState {
  const { t } = useTranslation();
  const ipcNames = { ...DEFAULT_IPC, ...ipc };

  const [status, setStatus] = useState<TaskStatus>('idle');
  const [step, setStep] = useState(0);
  const [totalSteps, setTotalSteps] = useState(0);
  const [stepName, setStepName] = useState('');
  const [error, setErrorState] = useState<string | null>(null);
  const [hasResult, setHasResult] = useState(false);
  const [logContent, setLogContent] = useState('');
  const [toast, setToast] = useState('');
  const logEndRef = useRef<HTMLDivElement>(null!);

  const statusRef = useRef<TaskStatus>('idle');
  statusRef.current = status;
  const logOffsetRef = useRef<number>(0);

  // Auto-scroll
  const autoScrollRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  useEffect(() => {
    const container = logEndRef.current?.parentElement;
    if (!container) return;
    const onScroll = () => {
      const { scrollTop, scrollHeight, clientHeight } = container;
      if (scrollTop < lastScrollTopRef.current) autoScrollRef.current = false;
      if (scrollHeight - scrollTop - clientHeight < 30) autoScrollRef.current = true;
      lastScrollTopRef.current = scrollTop;
    };
    container.addEventListener('scroll', onScroll);
    return () => container.removeEventListener('scroll', onScroll);
  }, []);
  useEffect(() => {
    if (!logContent) return;
    if (autoScrollRef.current) {
      requestAnimationFrame(() => {
        const container = logEndRef.current?.parentElement;
        if (container) container.scrollTop = container.scrollHeight;
      });
    }
  }, [logContent]);

  // lastEventAt tracking to discard stale snapshots (seed/event race guard)
  const lastEventAtRef = useRef<number>(0);

  // Apply Task snapshot to React state
  const applyTask = useCallback((task: TaskSnapshot | null) => {
    if (!task) {
      lastEventAtRef.current = 0;
      setStatus('idle');
      setStep(0);
      setTotalSteps(0);
      setStepName('');
      setErrorState(null);
      setHasResult(false);
      return;
    }
    const at = typeof task.lastEventAt === 'number' ? task.lastEventAt : 0;
    if (at < lastEventAtRef.current) return;  // stale, discard
    lastEventAtRef.current = at;
    setStatus(task.status);
    setStep(task.step);
    setTotalSteps(task.totalSteps);
    setStepName(task.stepName);
    setErrorState(task.error);
    setHasResult(task.result != null);
  }, []);

  // Listen first, then fetch initial snapshot — events during get_task window are captured.
  useEffect(() => {
    lastEventAtRef.current = 0;
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    (async () => {
      const fn = await listen<TaskSnapshot>('subprocess://task', e => {
        if (e.payload.taskId !== taskId) return;
        applyTask(e.payload);
      });
      if (disposed) { fn(); return; }
      unlisten = fn;
      try {
        const task = await invoke<TaskSnapshot | null>('get_task', { taskId });
        if (!disposed) applyTask(task);
      } catch {
        if (!disposed) applyTask(null);
      }
    })();
    return () => { disposed = true; unlisten?.(); };
  }, [taskId, applyTask]);

  // Poll log file for display (separate from state)
  useEffect(() => {
    const loadLog = async () => {
      try {
        const full = await invoke<string>(ipcNames.getLog, { taskId });
        setLogContent(full.slice(logOffsetRef.current));
      } catch {}
    };

    loadLog();
    if (status === 'running' || status === 'paused') {
      const interval = setInterval(loadLog, 2000);
      return () => clearInterval(interval);
    }
    if (status === 'done' || status === 'failed' || status === 'aborted') {
      // Final log flush after brief delay
      const t1 = setTimeout(loadLog, 300);
      const t2 = setTimeout(loadLog, 1000);
      return () => { clearTimeout(t1); clearTimeout(t2); };
    }
  }, [taskId, status, ipcNames.getLog]);

  const handlePauseResume = useCallback(async () => {
    try {
      if (status === 'paused') {
        await invoke(ipcNames.resume, { taskId });
      } else {
        await invoke(ipcNames.pause, { taskId });
      }
      // Status will be updated via subprocess://task event
    } catch (e) {
      console.error('Pause/resume failed:', e);
    }
  }, [taskId, status, ipcNames.pause, ipcNames.resume]);

  const handleAbort = useCallback(async () => {
    try {
      await invoke(ipcNames.abort, { taskId });
    } catch (e) {
      console.error('Abort failed:', e);
    }
  }, [taskId, ipcNames.abort]);

  const handleClearLog = useCallback(async () => {
    try {
      await invoke(ipcNames.clearLog, { taskId });
      setLogContent('');
      setToast(t('task.logCleared', 'Log cleared'));
      setTimeout(() => setToast(''), 1500);
    } catch {
      setToast(t('task.clearFailed', 'Clear failed'));
      setTimeout(() => setToast(''), 1500);
    }
  }, [taskId, ipcNames.clearLog, t]);

  const resetForNewRun = useCallback(async () => {
    // Mark log offset so previous log lines don't show in fresh run.
    try {
      const full = await invoke<string>(ipcNames.getLog, { taskId });
      logOffsetRef.current = full.length;
    } catch {
      logOffsetRef.current = 0;
    }
    setLogContent('');
    // Preempt status to 'running' so retry button disappears immediately.
    // Rust's first snapshot on spawn will override (idempotent).
    setStatus('running');
    setErrorState(null);
    setHasResult(false);
    lastEventAtRef.current = 0;
  }, [taskId, ipcNames.getLog]);

  const setError = useCallback((msg: string | null) => {
    setErrorState(msg);
    if (msg) setStatus('failed');
  }, []);

  // Derived: percent from status + step/totalSteps
  const percent = status === 'done'
    ? 100
    : totalSteps > 0
      ? Math.round((step / totalSteps) * 100)
      : 0;

  return {
    status, step, totalSteps, stepName, percent, error, hasResult,
    logContent, toast, logEndRef,
    handlePauseResume, handleAbort, handleClearLog, resetForNewRun,
    setError,
  };
}
