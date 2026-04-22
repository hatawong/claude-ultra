import { create } from 'zustand';
import i18n from '../i18n';
import type { AppConfig } from '../types/config';
import * as configService from '../services/configService';

interface ConfigState {
  config: AppConfig | null;
  loading: boolean;
  error: string | null;

  loadConfig: () => Promise<void>;
  saveConfig: (config: AppConfig, silent?: boolean) => Promise<void>;
  updateTheme: (theme: string) => Promise<void>;
  updateLanguage: (language: string) => Promise<void>;
  isMenuItemHidden: (path: string) => boolean;
}

export const useConfigStore = create<ConfigState>((set, get) => ({
  config: null,
  loading: false,
  error: null,

  loadConfig: async () => {
    set({ loading: true, error: null });
    try {
      const config = await configService.loadConfig();
      set({ config, loading: false });
      // Sync language between config and i18next
      const configLang = config.ui.language;
      const detectedLang = i18n.language;
      if (configLang && configLang !== detectedLang) {
        // Config has an explicit language set by user — use it
        i18n.changeLanguage(configLang);
      }
    } catch (error) {
      set({ error: String(error), loading: false });
    }
  },

  saveConfig: async (config: AppConfig, silent: boolean = false) => {
    if (!silent) set({ loading: true, error: null });
    try {
      await configService.saveConfig(config);
      set({ config, loading: false });
    } catch (error) {
      set({ error: String(error), loading: false });
      throw error;
    }
  },

  updateTheme: async (theme: string) => {
    const { config } = get();
    if (!config || config.ui.theme === theme) return;
    const newConfig = { ...config, ui: { ...config.ui, theme } };
    await get().saveConfig(newConfig, true);
  },

  updateLanguage: async (language: string) => {
    const { config } = get();
    if (!config || config.ui.language === language) return;
    const newConfig = { ...config, ui: { ...config.ui, language } };
    await get().saveConfig(newConfig, true);
  },

  isMenuItemHidden: (path: string) => {
    const { config } = get();
    if (!config) return false;
    return (config.ui.hidden_menu_items || []).includes(path);
  },
}));
