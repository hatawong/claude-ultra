import { invoke } from '@tauri-apps/api/core';
import type { Account } from '../types/account';

export async function listAccounts(): Promise<Account[]> {
  const accounts = await invoke<Account[]>('list_accounts');
  return accounts || [];
}

export async function deleteAccount(id: string): Promise<void> {
  await invoke('delete_account', { id });
}

export async function updateAccountLabel(id: string, label: string): Promise<void> {
  await invoke('update_account_label', { id, label });
}

export async function toggleUserStatus(id: string, enable: boolean): Promise<void> {
  await invoke('toggle_user_status', { id, enable });
}

export interface ImportResult {
  success: number;
  skipped: number;
  failed: number;
}

export async function importAccounts(json: string): Promise<ImportResult> {
  return await invoke<ImportResult>('import_accounts', { json });
}

export async function reorderAccounts(ids: string[]): Promise<void> {
  await invoke('reorder_accounts', { ids });
}

// ── Add Account (two-step: login → oauth) ──────────────────

export interface AddAccountResult {
  accountId: string;
  taskId: string;
}

export interface OAuthTokenData {
  accessToken: string;
  refreshToken: string;
  expiresAt: number;
  scopes?: string;
  cookies?: any[];
  sessionKey?: string;
}

export interface FinalizeResult {
  email: string;
  subscriptionType: string;
  proxyIp: string | null;
}

/** Create minimal account JSON + spawn web login subprocess */
export async function addAccountAndLogin(): Promise<AddAccountResult> {
  return await invoke<AddAccountResult>('add_account_and_login');
}

/** Start OAuth subprocess for an existing account */
export async function startOAuth(accountId: string): Promise<string> {
  return await invoke<string>('start_oauth', { accountId });
}

/** Start web login subprocess for an existing account */
export async function startWebLogin(accountId: string): Promise<string> {
  return await invoke<string>('start_web_login', { accountId });
}

/** Step final: get_profile + allocate proxy + hot-load into gateway */
export async function finalizeAccountImport(
  accountId: string,
  email: string,
  token: OAuthTokenData,
): Promise<FinalizeResult> {
  return await invoke<FinalizeResult>('finalize_account_import', { accountId, email, token });
}
