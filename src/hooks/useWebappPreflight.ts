/**
 * useWebappPreflight — check bun/Chrome/webapp availability before starting a task.
 *
 * Only runs when shouldRun=true (i.e., the "Start" button would be visible).
 * Returns { ready, issues } — if ready=true, issues is empty and nothing displays.
 */
import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface PreflightResult {
  ready: boolean;
  issues: string[];
}

const IDLE_RESULT: PreflightResult = { ready: true, issues: [] };

export function useWebappPreflight(shouldRun: boolean): PreflightResult {
  const [result, setResult] = useState<PreflightResult>(IDLE_RESULT);

  useEffect(() => {
    if (!shouldRun) {
      setResult(IDLE_RESULT);
      return;
    }
    invoke<PreflightResult>('check_webapp_ready')
      .then(setResult)
      .catch(() => setResult(IDLE_RESULT));
  }, [shouldRun]);

  return result;
}
