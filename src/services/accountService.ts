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

