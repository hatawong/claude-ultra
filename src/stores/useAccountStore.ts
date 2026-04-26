import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Account, Utilization, StaticProxy, ProxyTestResult } from '../types/account';
import {
  listAccounts,
  deleteAccount as deleteAccountApi,
  updateAccountLabel as updateLabelApi,
  toggleUserStatus as toggleUserApi,
  importAccounts as importAccountsApi,
  reorderAccounts as reorderAccountsApi,
} from '../services/accountService';
import type { ImportResult } from '../services/accountService';

interface AccountState {
  accounts: Account[];
  loading: boolean;
  error: string | null;
  selectedIds: Set<string>;
  refreshingQuota: Set<string>;
  /** accountId for in-progress Add Account flow (survives dialog close + page navigation) */
  addingAccountId: string | null;
  setAddingAccountId: (id: string | null) => void;
  /** staticProxy captured during in-progress Add flow (parent state) */
  addingStaticProxy: StaticProxy | null;
  setAddingStaticProxy: (sp: StaticProxy | null) => void;
  /** Last test result for the in-progress Add flow's staticProxy */
  addingTestResult: ProxyTestResult | null;
  setAddingTestResult: (tr: ProxyTestResult | null) => void;
  fetchAccounts: () => Promise<void>;
  deleteAccount: (id: string) => Promise<void>;
  updateAccountLabel: (id: string, label: string) => Promise<void>;
  toggleUserStatus: (id: string, enable: boolean) => Promise<void>;
  importAccounts: (json: string) => Promise<ImportResult>;
  reorderAccounts: (ids: string[]) => Promise<void>;
  refreshQuota: (accountId: string) => Promise<void>;
  setSelectedIds: (ids: Set<string>) => void;
  toggleSelected: (id: string) => void;
  selectAll: (ids: string[]) => void;
  clearSelection: () => void;
}

export const useAccountStore = create<AccountState>((set, get) => ({
  accounts: [],
  loading: false,
  error: null,
  selectedIds: new Set<string>(),
  refreshingQuota: new Set<string>(),
  addingAccountId: null,
  setAddingAccountId: (id) => set({ addingAccountId: id }),
  addingStaticProxy: null,
  setAddingStaticProxy: (sp) => set({ addingStaticProxy: sp }),
  addingTestResult: null,
  setAddingTestResult: (tr) => set({ addingTestResult: tr }),

  fetchAccounts: async () => {
    set({ loading: true, error: null });
    try {
      const accounts = await listAccounts();
      set({ accounts, loading: false });
    } catch (error) {
      set({ error: String(error), loading: false });
    }
  },

  deleteAccount: async (id: string) => {
    await deleteAccountApi(id);
    set((state) => ({
      accounts: state.accounts.filter((a) => a.accountId !== id),
      selectedIds: new Set([...state.selectedIds].filter((sid) => sid !== id)),
    }));
  },

  updateAccountLabel: async (id: string, label: string) => {
    await updateLabelApi(id, label);
    set((state) => ({
      accounts: state.accounts.map((a) =>
        a.accountId === id ? { ...a, customLabel: label || null } : a,
      ),
    }));
  },

  toggleUserStatus: async (id: string, enable: boolean) => {
    await toggleUserApi(id, enable);
    set((state) => ({
      accounts: state.accounts.map((a) =>
        a.accountId === id
          ? {
              ...a,
              userDisabled: !enable,
              userDisabledReason: enable ? null : 'Disabled manually by user',
            }
          : a,
      ),
    }));
  },

  importAccounts: async (json: string) => {
    const result = await importAccountsApi(json);
    if (result.success > 0) {
      await get().fetchAccounts();
    }
    return result;
  },

  reorderAccounts: async (ids: string[]) => {
    await reorderAccountsApi(ids);
    set((state) => {
      const accountMap = new Map(state.accounts.map((a) => [a.accountId, a]));
      const reordered = ids
        .map((id) => accountMap.get(id))
        .filter(Boolean) as Account[];
      // Add any accounts not in the ids list at the end
      const remaining = state.accounts.filter((a) => !ids.includes(a.accountId));
      return { accounts: [...reordered, ...remaining] };
    });
  },

  refreshQuota: async (accountId: string) => {
    set((state) => ({ refreshingQuota: new Set(state.refreshingQuota).add(accountId) }));
    try {
      const usage = await invoke<Utilization>('get_account_quota', { accountId });
      set((state) => ({
        accounts: state.accounts.map((a) =>
          a.accountId === accountId ? { ...a, utilization: usage } : a
        ),
      }));
    } finally {
      set((state) => {
        const next = new Set(state.refreshingQuota);
        next.delete(accountId);
        return { refreshingQuota: next };
      });
    }
  },

  setSelectedIds: (ids: Set<string>) => set({ selectedIds: ids }),

  toggleSelected: (id: string) =>
    set((state) => {
      const next = new Set(state.selectedIds);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return { selectedIds: next };
    }),

  selectAll: (ids: string[]) => set({ selectedIds: new Set(ids) }),

  clearSelection: () => set({ selectedIds: new Set() }),
}));
