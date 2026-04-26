/**
 * StaticProxyEditPanel — Shared UI for editing a static residential proxy.
 *
 * Used in two contexts via the `mode` prop:
 * - mode='add': inside AddAccountDialog's child dialog. Test calls dryrun
 *   IPC (no accountId, no disk write). Save returns sp + testResult to
 *   parent so AddAccountDialog can forward both to add_account_and_login
 *   for atomic write at submit time.
 * - mode='edit': inside AccountDetailsDialog::RouteEditor. Test calls the
 *   connection IPC (accountId, same-creds refresh persistence). Save calls
 *   update_account_route directly. Delete shows ConfirmDialog and clears.
 *
 * Both modes share the same UI (paste row + 5 fields + action buttons +
 * test result line), guarding against drift between Add and Edit flows.
 */
import { useState, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import type { StaticProxy, ProxyTestResult } from '../../types/account';
import { parseStaticProxyPasted } from '../../utils/staticProxyParser';
import { cn } from '../../utils/cn';
import ConfirmDialog from '../common/ConfirmDialog';

interface StaticProxyEditPanelProps {
  mode: 'add' | 'edit';
  accountId?: string;
  initialStaticProxy: StaticProxy | null;
  initialTestResult?: ProxyTestResult | null;
  onSave: (sp: StaticProxy, testResult: ProxyTestResult) => void;
  onDelete?: () => void;
  onUpdated?: () => void;
  onToast: (msg: string) => void;
  saving?: boolean;
}

export default function StaticProxyEditPanel({
  mode, accountId, initialStaticProxy, initialTestResult,
  onSave, onDelete, onUpdated, onToast, saving: parentSaving,
}: StaticProxyEditPanelProps) {
  const { t } = useTranslation();
  const [sp, setSp] = useState<StaticProxy | null>(initialStaticProxy);
  const [pasted, setPasted] = useState('');
  const [testResult, setTestResult] = useState<ProxyTestResult | null>(initialTestResult || null);
  // testedKey = JSON.stringify(sp) at the time the most recent OK Test
  // returned. Save is gated on `testedKey === currentKey`. Without this
  // gate, a slow Test request can return after the user has already
  // edited fields and let Save commit unverified credentials with a
  // stale ProxySection snapshot.
  const [testedKey, setTestedKey] = useState<string | null>(
    initialStaticProxy && initialTestResult?.ok ? JSON.stringify(initialStaticProxy) : null,
  );
  const [testing, setTesting] = useState(false);
  // Sequence counter to discard out-of-order Test responses (user clicks
  // Test twice quickly with different inputs in between).
  const testSeqRef = useRef(0);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [savingLocal, setSavingLocal] = useState(false);
  const saving = parentSaving || savingLocal;

  // mode='edit': resync local sp when initialStaticProxy content changes.
  // Using JSON.stringify in deps so fetchAccounts() returning a new Account
  // ref with identical content does not retrigger this effect and overwrite
  // user's in-progress edits.
  useEffect(() => {
    if (mode === 'edit') {
      setSp(initialStaticProxy);
    }
  }, [mode, JSON.stringify(initialStaticProxy)]);

  const handlePaste = () => {
    const parsed = parseStaticProxyPasted(pasted);
    if (parsed) {
      setSp(parsed);
      setPasted('');
      setTestResult(null);
      setTestedKey(null);
    } else {
      onToast(t('accounts.static_proxy.parse_failed', 'Invalid credential format'));
    }
  };

  const handleFieldChange = (patch: Partial<StaticProxy>) => {
    setSp((prev) => ({
      protocol: prev?.protocol ?? 'socks5',
      host: prev?.host ?? '',
      port: prev?.port ?? 0,
      username: prev?.username ?? '',
      password: prev?.password ?? '',
      ...patch,
    }));
    setTestResult(null);
    setTestedKey(null);
  };

  const isStaticComplete = !!(sp && sp.host && sp.port && sp.username && sp.password);
  const currentKey = sp ? JSON.stringify(sp) : null;

  const handleTest = async () => {
    if (!isStaticComplete || !sp) return;
    const candidate = sp;
    const candidateKey = JSON.stringify(candidate);
    const seq = ++testSeqRef.current;
    setTesting(true);
    setTestResult(null);
    setTestedKey(null);
    try {
      const ipc = mode === 'add' ? 'test_static_proxy_dryrun' : 'test_static_proxy_connection';
      const args = mode === 'add'
        ? { staticProxy: candidate }
        : { accountId, staticProxy: candidate };
      const result = await invoke<ProxyTestResult>(ipc, args);
      // Discard if a newer Test was started or the input has changed.
      if (seq !== testSeqRef.current) return;
      setTestResult(result);
      if (result.ok) setTestedKey(candidateKey);
      if (result.ok && mode === 'edit') {
        // Backend wrote a ProxySection snapshot for same-creds refresh; reload
        // account so RouteTooltip / AccountTable show new IP/Country/ISP.
        onUpdated?.();
      }
    } catch (e) {
      if (seq !== testSeqRef.current) return;
      setTestResult({ ok: false, mode: 'proxied', ip: null, country: null, error: String(e) });
    } finally {
      if (seq === testSeqRef.current) setTesting(false);
    }
  };

  const handleSave = async () => {
    if (!isStaticComplete || !sp || !testResult?.ok) return;
    // Reject Save when the current input no longer matches what was tested.
    // This catches stale async results that landed after the user edited
    // fields, preventing commit of unverified credentials with a stale
    // ProxySection snapshot.
    if (testedKey !== currentKey) return;
    if (mode === 'add') {
      onSave(sp, testResult);
      return;
    }
    setSavingLocal(true);
    try {
      const probedProxy = testResult?.ok ? testResult.proxySection ?? null : null;
      await invoke('update_account_route', {
        accountId,
        routeMode: 'static',
        staticProxy: sp,
        probedProxy,
      });
      onToast(t('accounts.details.route_updated', 'Route updated'));
      onUpdated?.();
    } catch (e) {
      onToast(String(e));
    } finally {
      setSavingLocal(false);
    }
  };

  const handleDeleteClick = () => setShowDeleteConfirm(true);

  const performDelete = async () => {
    setShowDeleteConfirm(false);
    if (mode === 'add') {
      onDelete?.();
      return;
    }
    setSavingLocal(true);
    try {
      await invoke('update_account_route', {
        accountId,
        routeMode: 'proxy',
        clearStaticProxy: true,
      });
      setSp(null);
      setTestResult(null);
      onToast(t('accounts.details.route_updated', 'Route updated'));
      onUpdated?.();
    } catch (e) {
      onToast(String(e));
    } finally {
      setSavingLocal(false);
    }
  };

  return (
    <div className="flex flex-col gap-1.5">
      {/* Paste row */}
      <div className="flex items-center gap-1.5">
        <input
          type="text"
          value={pasted}
          onChange={(e) => setPasted(e.target.value)}
          placeholder={t('accounts.static_proxy.paste_placeholder', 'Format: {{protocol}}://host:port:user:pass', { protocol: sp?.protocol || 'socks5' })}
          className="flex-1 text-xs bg-base-100 border border-base-300 rounded px-2 py-0.5 text-base-content"
        />
        <button
          onClick={handlePaste}
          disabled={!pasted}
          className="px-2 py-0.5 text-xs bg-blue-500 text-white rounded disabled:bg-gray-600 disabled:cursor-not-allowed"
        >
          {t('accounts.static_proxy.paste', 'Parse')}
        </button>
      </div>
      {/* 5 fields */}
      <div className="grid grid-cols-2 gap-1.5">
        <select
          value={sp?.protocol || 'socks5'}
          onChange={(e) => handleFieldChange({ protocol: e.target.value as 'socks5' | 'http' })}
          className="text-xs bg-base-100 border border-base-300 rounded px-2 py-0.5 text-base-content"
        >
          <option value="socks5">SOCKS5</option>
          <option value="http">HTTP</option>
        </select>
        <input
          type="text"
          placeholder={t('accounts.static_proxy.host', 'Host')}
          value={sp?.host || ''}
          onChange={(e) => handleFieldChange({ host: e.target.value })}
          className="text-xs bg-base-100 border border-base-300 rounded px-2 py-0.5 text-base-content"
        />
        <input
          type="number"
          min={1}
          max={65535}
          placeholder={t('accounts.static_proxy.port', 'Port')}
          value={sp?.port || ''}
          onChange={(e) => {
            // Clamp to valid u16 port range. parseInt ?? 0 alone allows
            // negatives and values >65535, which Rust deserialization
            // would reject with an opaque error.
            const n = Number.parseInt(e.target.value, 10);
            const port = Number.isFinite(n) && n >= 1 && n <= 65535 ? n : 0;
            handleFieldChange({ port });
          }}
          className="text-xs bg-base-100 border border-base-300 rounded px-2 py-0.5 text-base-content"
        />
        <input
          type="text"
          placeholder={t('accounts.static_proxy.username', 'Username')}
          value={sp?.username || ''}
          onChange={(e) => handleFieldChange({ username: e.target.value })}
          className="text-xs bg-base-100 border border-base-300 rounded px-2 py-0.5 text-base-content"
        />
        <input
          type="password"
          placeholder={t('accounts.static_proxy.password', 'Password')}
          value={sp?.password || ''}
          onChange={(e) => handleFieldChange({ password: e.target.value })}
          className="text-xs bg-base-100 border border-base-300 rounded px-2 py-0.5 text-base-content col-span-2"
        />
      </div>
      {/* Action buttons */}
      <div className="flex items-center gap-1.5">
        <button
          onClick={handleTest}
          disabled={!isStaticComplete || testing || saving}
          className="px-2 py-0.5 text-xs bg-blue-500 text-white rounded disabled:bg-gray-600 disabled:cursor-not-allowed"
        >
          {testing ? t('common.loading', 'Loading...') : t('accounts.static_proxy.test', 'Test')}
        </button>
        <button
          onClick={handleSave}
          disabled={!isStaticComplete || saving || !testResult?.ok || testedKey !== currentKey}
          className="px-2 py-0.5 text-xs bg-green-500 text-white rounded disabled:bg-gray-600 disabled:cursor-not-allowed"
          title={!testResult?.ok ? t('accounts.static_proxy.must_test_first', 'Test the connection first') : undefined}
        >
          {t('common.save', 'Save')}
        </button>
        {(initialStaticProxy || sp) && (
          <button
            onClick={handleDeleteClick}
            disabled={saving}
            className="px-2 py-0.5 text-xs bg-red-500 text-white rounded disabled:bg-gray-600"
          >
            {t('accounts.static_proxy.delete', 'Delete')}
          </button>
        )}
      </div>
      {/* Test result */}
      {testResult && (
        <div className={cn('text-xs', testResult.ok ? 'text-green-600 dark:text-green-400' : 'text-red-600 dark:text-red-400')}>
          {testResult.ok
            ? t('proxy.test_proxied', { ip: testResult.ip, country: testResult.country })
            : t('proxy.test_failed', { error: testResult.error || 'Unknown error' })}
        </div>
      )}
      {showDeleteConfirm && (
        <ConfirmDialog
          title={t('accounts.static_proxy.delete', 'Delete')}
          message={t('accounts.static_proxy.delete_confirm', 'Delete static proxy credentials? Route mode will fall back to Proxy.')}
          confirmText={t('accounts.static_proxy.delete', 'Delete')}
          confirmColor="red"
          onConfirm={performDelete}
          onCancel={() => setShowDeleteConfirm(false)}
        />
      )}
    </div>
  );
}
