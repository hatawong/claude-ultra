import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { AuthStatus } from '../types/auth';

interface AuthStore {
  authStatus: AuthStatus | null;
  loading: boolean;
  fetchAuthStatus: () => Promise<AuthStatus>;
  setAuthStatus: (status: AuthStatus) => void;
  logout: () => Promise<void>;
}

export const useAuthStore = create<AuthStore>((set) => ({
  authStatus: null,
  loading: true,

  fetchAuthStatus: async () => {
    set({ loading: true });
    try {
      const status = await invoke<AuthStatus>('get_auth_status');
      set({ authStatus: status, loading: false });
      return status;
    } catch (e) {
      set({ loading: false });
      throw e;
    }
  },

  setAuthStatus: (status) => set({ authStatus: status }),

  logout: async () => {
    await invoke('logout');
    set({
      authStatus: {
        mode: 'distribution',
        status: 'not_logged_in',
        user: undefined,
      },
    });
  },
}));
