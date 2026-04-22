/**
 * useTaskDialog — subprocess task dialog logic.
 *
 * Provides: event listening, log polling, progress recovery, pause/resume, auto-scroll.
 */
import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';

export type TaskStatus = 'idle' | 'running' | 'paused' | 'done' | 'failed';

export interface TaskDialogState {
  status: TaskStatus;
  step: number;
  totalSteps: number;
  stepName: string;
  percent: number;
  error: string | null;
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
  initialStatus?: TaskStatus;
  ipc?: Partial<IpcNames>;
  onEvent?: (eventType: string, data: any) => void;
  onDone?: () => void;
  // TODO (P3-D): deprecate unless wired up. Currently only called from 2 of 15
  // setStatus sites (pause/resume). No component subscribes as of v1.0.1 — either
  // cover all transitions (setStatusWithNotify helper) or remove the option when
  // a real consumer appears.
  onStatusChange?: (status: TaskStatus) => void;
}

export function useTaskDialog({ taskId, initialStatus, ipc, onEvent, onDone, onStatusChange }: UseTaskDialogOptions): TaskDialogState {
  const { t } = useTranslation();
  const ipcNames = { ...DEFAULT_IPC, ...ipc };
  const [status, setStatus] = useState<TaskStatus>(initialStatus || 'idle');

  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;
  const onStatusChangeRef = useRef(onStatusChange);
  onStatusChangeRef.current = onStatusChange;

  const [step, setStep] = useState(0);
  const [totalSteps, setTotalSteps] = useState(0);
  const [stepName, setStepName] = useState('');
  const [percent, setPercent] = useState(0);
  const [error, setError] = useState<string | null>(null);
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

  // On mount: load existing log, restore progress
  useEffect(() => {
    (async () => {
      try {
        const content = await invoke<string>(ipcNames.getLog, { taskId });
        if (content) {
          setLogContent(content.slice(logOffsetRef.current));
          const stepMatches = content.matchAll(/\[Step (\d+)\/(\d+)\]\s*(.+)/g);
          let lastMatch: RegExpMatchArray | null = null;
          for (const m of stepMatches) lastMatch = m;
          if (lastMatch) {
            setStep(parseInt(lastMatch[1]));
            setTotalSteps(parseInt(lastMatch[2]));
            setStepName(lastMatch[3].trim());
            setPercent(Math.round((parseInt(lastMatch[1]) / parseInt(lastMatch[2])) * 100));
          }
          const lastLines = content.trimEnd().split('\n').slice(-5).join('\n');
          if (lastLines.includes('失败') || lastLines.includes('suspended') || lastLines.includes('banned')) {
            setStatus('failed');
            const errMatch = lastLines.match(/❌\s*(.+失败.+)/);
            if (errMatch) setError(errMatch[1].trim());
          } else if (lastLines.includes('浏览器已关闭') || lastLines.includes('注册完成') || lastLines.includes('100%')) {
            setStatus('done');
            setPercent(100);
          } else if (lastMatch) {
            // Has progress but no terminal state → check if process is actually running
            try {
              const running = await invoke<boolean>('is_subprocess_running', { taskId });
              if (running) {
                const pauseIdx = content.lastIndexOf('⏸ 已暂停');
                const resumeIdx = content.lastIndexOf('▶ 已恢复');
                setStatus(pauseIdx > resumeIdx ? 'paused' : 'running');
              } else {
                // Process is dead, mark as failed
                setStatus('failed');
                setError('Process exited unexpectedly');
              }
            } catch {
              setStatus('failed');
              setError('Process exited unexpectedly');
            }
          }
        }
      } catch {}
    })();
  }, [taskId]);

  // Listen to subprocess events
  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const reg = async (promise: Promise<UnlistenFn>) => {
      const fn = await promise;
      if (disposed) fn();
      else unlisteners.push(fn);
    };

    const setup = async () => {
      await reg(listen<any>('subprocess://step', (event) => {
        const { taskId: tid, data } = event.payload;
        if (tid !== taskId) return;
        if (statusRef.current === 'idle' || statusRef.current === 'failed') setStatus('running');
        setStep(data.step);
        setTotalSteps(data.total);
        setStepName(data.name);
        setPercent(Math.round((data.step / data.total) * 100));
      }));

      await reg(listen<any>('subprocess://progress', (event) => {
        const { taskId: tid, data } = event.payload;
        if (tid !== taskId) return;
        if (statusRef.current === 'idle') setStatus('running');
        setPercent(data.percent);
      }));

      await reg(listen<any>('subprocess://log', (event) => {
        if (event.payload.taskId !== taskId) return;
        if (statusRef.current === 'idle') setStatus('running');
      }));

      await reg(listen<any>('subprocess://cookies', (event) => {
        const { taskId: tid, data } = event.payload;
        if (tid !== taskId) return;
        onEventRef.current?.('cookies', data);
      }));

      await reg(listen<any>('subprocess://meta', (event) => {
        const { taskId: tid, data } = event.payload;
        if (tid !== taskId) return;
        onEventRef.current?.('meta', data);
      }));

      await reg(listen<any>('subprocess://result', (event) => {
        const { taskId: tid, data } = event.payload;
        if (tid !== taskId) return;
        if (data.success) {
          setStatus('done');
          setPercent(100);
          onEventRef.current?.('result', data);
          onDoneRef.current?.();
        } else {
          setStatus('failed');
          setError(data.data?.error || 'Failed');
        }
      }));

      await reg(listen<any>('subprocess://error', (event) => {
        const { taskId: tid, data } = event.payload;
        if (tid !== taskId) return;
        setStatus('failed');
        setError(data.msg || data.code);
      }));

      await reg(listen<any>('subprocess://completed', (event) => {
        if (event.payload.taskId !== taskId) return;
        if (statusRef.current === 'running') {
          setStatus('done');
          setPercent(100);
          onDoneRef.current?.();
        }
      }));

      await reg(listen<any>('subprocess://failed', (event) => {
        if (event.payload.taskId !== taskId) return;
        setStatus('failed');
        setError(event.payload.error || 'Process exited with error');
      }));
    };

    setup();
    return () => { disposed = true; unlisteners.forEach((fn) => fn()); };
  }, [taskId]);

  // Poll log file
  useEffect(() => {
    const loadLog = async () => {
      try {
        const full = await invoke<string>(ipcNames.getLog, { taskId });
        setLogContent(full.slice(logOffsetRef.current));
      } catch {}
    };

    if (status === 'running' || status === 'paused') {
      loadLog();
      const interval = setInterval(loadLog, 2000);
      return () => clearInterval(interval);
    }
    if (status === 'done' || status === 'failed') {
      const t1 = setTimeout(loadLog, 300);
      const t2 = setTimeout(loadLog, 1000);
      const t3 = setTimeout(loadLog, 2000);
      return () => { clearTimeout(t1); clearTimeout(t2); clearTimeout(t3); };
    }
  }, [taskId, status]);

  const handlePauseResume = useCallback(async () => {
    try {
      if (status === 'paused') {
        await invoke(ipcNames.resume, { taskId });
        setStatus('running');
        onStatusChangeRef.current?.('running');
      } else {
        await invoke(ipcNames.pause, { taskId });
        setStatus('paused');
        onStatusChangeRef.current?.('paused');
      }
    } catch (e) {
      console.error('Pause/resume failed:', e);
    }
  }, [taskId, status]);

  const handleAbort = useCallback(async () => {
    try {
      await invoke(ipcNames.abort, { taskId });
    } catch (e) {
      console.error('Abort failed:', e);
    }
  }, [taskId]);

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
  }, [taskId]);

  const resetForNewRun = useCallback(async () => {
    try {
      const full = await invoke<string>(ipcNames.getLog, { taskId });
      logOffsetRef.current = full.length;
    } catch {
      logOffsetRef.current = 0;
    }
    setError(null);
    setStatus('running');
    setStep(0);
    setTotalSteps(0);
    setStepName('');
    setPercent(0);
    setLogContent('');
  }, [taskId]);

  return {
    status, step, totalSteps, stepName, percent, error,
    logContent, toast, logEndRef,
    handlePauseResume, handleAbort, handleClearLog, resetForNewRun,
    setError: (msg: string | null) => { setError(msg); if (msg) setStatus('failed'); },
  };
}
