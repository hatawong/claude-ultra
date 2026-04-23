import { useState, useEffect } from 'react';
import {
  Save, Github, User, MessageCircle, ExternalLink,
  RefreshCw, Heart, Send, LogOut, Key,
  LayoutDashboard, Users, Network, Activity,
  BarChart3, Settings as SettingsIcon, Lock, CheckCircle2,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { useConfigStore } from '../stores/useConfigStore';
import { useAuthStore } from '../stores/useAuthStore';
import type { AppConfig } from '../types/config';
import type { AuthUser } from '../types/auth';
import { showToast } from '../components/common/Toast';
import { enable as enableAutostart, disable as disableAutostart, isEnabled as isAutostartEnabled } from '@tauri-apps/plugin-autostart';
import { useSearchParams } from 'react-router-dom';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

type SettingsTab = 'general' | 'proxy' | 'account' | 'about';

function Settings() {
  const { t, i18n } = useTranslation();
  const { config, loadConfig, saveConfig } = useConfigStore();
  const [searchParams] = useSearchParams();
  const initialTab = (searchParams.get('tab') as SettingsTab) || 'general';
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab);

  // Sync tab from URL search params
  const tabParam = searchParams.get('tab');
  useEffect(() => {
    if (tabParam && tabParam !== activeTab) {
      setActiveTab(tabParam as SettingsTab);
    }
  }, [tabParam]);

  // Listen for Navbar user badge click (custom event, works when already on /settings)
  useEffect(() => {
    const handler = (e: Event) => {
      const tab = (e as CustomEvent).detail as SettingsTab;
      if (tab) setActiveTab(tab);
    };
    window.addEventListener('settings-tab', handler);
    return () => window.removeEventListener('settings-tab', handler);
  }, []);

  // formData is the editing copy — only synced to backend on Save
  const { authStatus, logout } = useAuthStore();

  const [formData, setFormData] = useState<AppConfig>({
    ui: {
      language: 'en',
      theme: 'dark',
      auto_launch: false,
      auto_check_update: true,
      update_check_interval: 24,
      hidden_menu_items: [],
    },
    server_url: 'http://localhost:8787',
    gateway: {} as any,
    proxy: { default_type: 'residential', residential: { host: 'geo.iproyal.com', port: 12321, username: '', password: '', default_country: 'us' }, mobile: { username: '', password: '', host: '', http_port: 0, socks5_port: 0, rotate_url: '' } },
    email: { domains: [], mailslurp_api_key: '', mailslurp_inbox_id: '' },
    sms: { api_key: '' },
    registration: {
      supported_countries: {
        us: { sms_max_price: 0.51 },
        ph: { sms_max_price: 0.51 },
      },
    },
  });

  const [isCheckingUpdate, setIsCheckingUpdate] = useState(false);

  const checkForUpdates = async () => {
    setIsCheckingUpdate(true);
    try {
      const update = await check();
      if (update) {
        const confirmed = window.confirm(
          `New version v${update.version} available. Download and install?`
        );
        if (confirmed) {
          await update.downloadAndInstall();
          await relaunch();
        }
      } else {
        showToast(t('settings.about.up_to_date', 'Already up to date'), 'success');
      }
    } catch (e) {
      showToast(`Update check failed: ${e}`, 'error');
    } finally {
      setIsCheckingUpdate(false);
    }
  };

  // Load config on mount
  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  // Sync formData when config loads from backend, and sync autostart state
  useEffect(() => {
    if (config) {
      setFormData(config);
      isAutostartEnabled().then((enabled) => {
        if (enabled !== config.ui.auto_launch) {
          setFormData((prev) => ({ ...prev, ui: { ...prev.ui, auto_launch: enabled } }));
        }
      }).catch(() => {});
    }
  }, [config]);

  // Save handler — writes formData to Rust backend
  const handleSave = async () => {
    try {
      await saveConfig(formData);
    } catch (error) {
      console.error('Save failed:', error);
    }
  };

  // Language — immediate effect (i18n + save)
  const handleLanguageChange = async (lang: string) => {
    const newFormData = { ...formData, ui: { ...formData.ui, language: lang } };
    setFormData(newFormData);
    i18n.changeLanguage(lang);
    document.documentElement.dir = lang === 'ar' ? 'rtl' : 'ltr';
    try {
      await saveConfig(newFormData, true);
    } catch (error) {
      console.error('Failed to save language:', error);
    }
  };

  // Theme — immediate effect
  const handleThemeChange = async (theme: string) => {
    const newFormData = { ...formData, ui: { ...formData.ui, theme } };
    setFormData(newFormData);
    localStorage.setItem('app-theme-preference', theme);
    const root = document.documentElement;
    if (theme === 'dark') {
      root.classList.add('dark');
      root.setAttribute('data-theme', 'dark');
      root.style.backgroundColor = '#15191e';
    } else {
      root.classList.remove('dark');
      root.setAttribute('data-theme', 'light');
      root.style.backgroundColor = '#FAFBFC';
    }
    try {
      await saveConfig(newFormData, true);
    } catch (error) {
      console.error('Failed to save theme:', error);
    }
  };

  // Menu visibility — immediate save
  const toggleMenuItem = async (path: string) => {
    const hidden = formData.ui.hidden_menu_items || [];
    const isVisible = !hidden.includes(path);
    const newHidden = isVisible ? [...hidden, path] : hidden.filter((p) => p !== path);
    const newFormData = { ...formData, ui: { ...formData.ui, hidden_menu_items: newHidden } };
    setFormData(newFormData);
    try {
      await saveConfig(newFormData, true);
    } catch (error) {
      console.error('Failed to save menu visibility:', error);
    }
  };

  const allTabs: { key: SettingsTab; labelKey: string; fallback: string }[] = [
    { key: 'general', labelKey: 'settings.tabs.general', fallback: 'General' },
    { key: 'proxy', labelKey: 'proxy.title', fallback: 'Proxy' },
    { key: 'account', labelKey: 'settings.tabs.account', fallback: 'Account' },
    { key: 'about', labelKey: 'settings.tabs.about', fallback: 'About' },
  ];
  const tabs = authStatus?.mode === 'internal'
    ? allTabs.filter(t => t.key !== 'account')
    : allTabs;

  const menuItems = [
    { path: '/', labelKey: 'nav.dashboard', fallback: 'Dashboard', icon: LayoutDashboard },
    { path: '/accounts', labelKey: 'nav.accounts', fallback: 'Accounts', icon: Users },
    { path: '/gateway', labelKey: 'nav.gateway', fallback: 'Gateway', icon: Network },
    { path: '/monitor', labelKey: 'nav.call_records', fallback: 'Monitor', icon: Activity },
    { path: '/token-stats', labelKey: 'nav.token_stats', fallback: 'Token Stats', icon: BarChart3 },
    { path: '/security', labelKey: 'nav.security', fallback: 'Security', icon: Lock },
    { path: '/settings', labelKey: 'nav.settings', fallback: 'Settings', icon: SettingsIcon },
  ];

  // Don't render until config is loaded — prevents placeholder from being saved to disk
  if (!config) {
    return <div className="h-full w-full flex items-center justify-center text-gray-400">{t('common.loading', 'Loading...')}</div>;
  }

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="p-5 space-y-4 max-w-7xl mx-auto">
        {/* Top: Tab nav + Save button */}
        <div className="flex justify-between items-center">
          <div className="flex items-center gap-0.5 bg-gray-100 dark:bg-base-200 rounded-full p-0.5 w-fit">
            {tabs.map((tab) => (
              <button
                key={tab.key}
                className={`px-4 py-1.5 rounded-full text-xs font-medium transition-all ${
                  activeTab === tab.key
                    ? 'bg-gray-200 dark:bg-gray-700 text-gray-900 dark:text-gray-100 shadow-sm'
                    : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-200'
                }`}
                onClick={() => setActiveTab(tab.key)}
              >
                {t(tab.labelKey, tab.fallback)}
              </button>
            ))}
          </div>

          <button
            className="px-3 py-1.5 bg-blue-500 text-white text-xs rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1.5 shadow-sm"
            onClick={handleSave}
          >
            <Save className="w-4 h-4" />
            {t('settings.save', 'Save')}
          </button>
        </div>

        {/* Settings form */}
        <div className="bg-white dark:bg-base-100 rounded-lg p-5 shadow-sm border border-gray-100 dark:border-base-200">
          {/* ── General ── */}
          {activeTab === 'general' && (
            <div className="space-y-6">
              <h2 className="text-lg font-semibold text-gray-900 dark:text-base-content">
                {t('settings.general.title', 'General Settings')}
              </h2>

              {/* Language */}
              <div>
                <label className="block text-sm font-medium text-gray-900 dark:text-base-content mb-2">
                  {t('settings.general.language', 'Language')}
                </label>
                <select
                  className="w-full px-4 py-4 border border-gray-200 dark:border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-gray-900 dark:text-base-content bg-gray-50 dark:bg-base-200"
                  value={formData.ui.language}
                  onChange={(e) => handleLanguageChange(e.target.value)}
                >
                  <option value="zh">简体中文</option>
                  <option value="zh-TW">繁體中文</option>
                  <option value="en">English</option>
                  <option value="ja">日本語</option>
                  <option value="tr">Türkçe</option>
                  <option value="vi">Tiếng Việt</option>
                  <option value="pt">Português</option>
                  <option value="ko">한국어</option>
                  <option value="ru">Русский</option>
                  <option value="ar">العربية</option>
                  <option value="es">Español</option>
                  <option value="my">Bahasa Melayu</option>
                </select>
              </div>

              {/* Theme */}
              <div>
                <label className="block text-sm font-medium text-gray-900 dark:text-base-content mb-2">
                  {t('settings.general.theme', 'Theme')}
                </label>
                <select
                  className="w-full px-4 py-4 border border-gray-200 dark:border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-gray-900 dark:text-base-content bg-gray-50 dark:bg-base-200"
                  value={formData.ui.theme}
                  onChange={(e) => handleThemeChange(e.target.value)}
                >
                  <option value="light">{t('settings.general.theme_light', 'Light')}</option>
                  <option value="dark">{t('settings.general.theme_dark', 'Dark')}</option>
                </select>
              </div>

              {/* Auto Launch */}
              <div className="flex items-center justify-between p-4 bg-gray-50 dark:bg-base-200 rounded-lg border border-gray-100 dark:border-base-300">
                <div>
                  <div className="font-medium text-gray-900 dark:text-base-content">
                    {t('settings.general.auto_launch', 'Auto Launch')}
                  </div>
                  <p className="text-sm text-gray-600 dark:text-gray-400 mt-1">
                    {t('settings.general.auto_launch_desc', 'Automatically start the app when you log in')}
                  </p>
                </div>
                <label className="relative inline-flex items-center cursor-pointer">
                  <input
                    type="checkbox"
                    className="sr-only peer"
                    checked={formData.ui.auto_launch}
                    onChange={async (e) => {
                      const enabled = e.target.checked;
                      try {
                        if (enabled) { await enableAutostart(); } else { await disableAutostart(); }
                        const newFormData = { ...formData, ui: { ...formData.ui, auto_launch: enabled } };
                        setFormData(newFormData);
                        await saveConfig(newFormData, true);
                      } catch (error) { console.error('Failed to toggle auto launch:', error); }
                    }}
                  />
                  <div className="w-9 h-5 bg-gray-200 dark:bg-base-300 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-500" />
                </label>
              </div>

              {/* Auto Check Update */}
              <div className="p-4 bg-gray-50 dark:bg-base-200 rounded-lg border border-gray-100 dark:border-base-300">
                <div className="flex items-center justify-between">
                  <div>
                    <div className="font-medium text-gray-900 dark:text-base-content">
                      {t('settings.general.auto_check_update', 'Auto Check Updates')}
                    </div>
                    <p className="text-sm text-gray-600 dark:text-gray-400 mt-1">
                      {t('settings.general.auto_check_update_desc', 'Periodically check for new versions')}
                    </p>
                  </div>
                  <label className="relative inline-flex items-center cursor-pointer">
                    <input
                      type="checkbox"
                      className="sr-only peer"
                      checked={formData.ui.auto_check_update}
                      onChange={(e) => setFormData({ ...formData, ui: { ...formData.ui, auto_check_update: e.target.checked } })}
                    />
                    <div className="w-9 h-5 bg-gray-200 dark:bg-base-300 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-500" />
                  </label>
                </div>
                {formData.ui.auto_check_update && (
                  <div className="flex items-center gap-3 mt-3 pt-3 border-t border-gray-200 dark:border-base-300">
                    <label className="text-sm text-gray-600 dark:text-gray-400">
                      {t('settings.general.update_check_interval', 'Check Interval (hours)')}
                    </label>
                    <input
                      type="number"
                      className="w-20 px-2.5 py-1.5 border border-gray-200 dark:border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-sm text-gray-900 dark:text-base-content bg-white dark:bg-base-100"
                      min="1"
                      max="168"
                      value={formData.ui.update_check_interval}
                      onChange={(e) => setFormData({ ...formData, ui: { ...formData.ui, update_check_interval: parseInt(e.target.value) || 24 } })}
                    />
                  </div>
                )}
              </div>

              {/* Menu Visibility */}
              <div className="border-t border-gray-200 dark:border-base-200 pt-6 mt-6">
                <h3 className="font-medium text-gray-900 dark:text-base-content mb-3">
                  {t('settings.menu.title', 'Menu Display')}
                </h3>
                <p className="text-sm text-gray-600 dark:text-gray-400 mb-4">
                  {t('settings.menu.desc', 'Choose which menu items to show in the navigation bar')}
                </p>
                <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
                  {menuItems.map((item) => {
                    const isVisible = !(formData.ui.hidden_menu_items || []).includes(item.path);
                    const isSettings = item.path === '/settings';
                    return (
                      <div
                        key={item.path}
                        onClick={() => !isSettings && toggleMenuItem(item.path)}
                        className={`
                          relative flex flex-col items-center justify-center gap-3 p-4 rounded-xl border-2 transition-all cursor-pointer select-none
                          ${isSettings
                            ? 'bg-gray-50 dark:bg-base-200 border-gray-100 dark:border-base-300 opacity-60 cursor-not-allowed'
                            : isVisible
                              ? 'bg-blue-50/50 dark:bg-blue-900/10 border-blue-500 dark:border-blue-500 shadow-sm'
                              : 'bg-white dark:bg-base-100 border-gray-200 dark:border-base-300 hover:border-gray-300 dark:hover:border-base-content/20 text-gray-500'
                          }
                        `}
                      >
                        {isVisible && (
                          <div className="absolute top-2 right-2 text-blue-500">
                            <CheckCircle2 size={16} fill="currentColor" className="text-white dark:text-base-100" />
                          </div>
                        )}
                        {isSettings && (
                          <div className="absolute top-2 right-2 text-xs font-bold text-gray-400 bg-gray-200 dark:bg-base-300 px-1.5 py-0.5 rounded">
                            {t('settings.menu.required', 'Required')}
                          </div>
                        )}
                        <div className={`p-3 rounded-xl transition-colors ${
                          isVisible
                            ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400'
                            : 'bg-gray-100 dark:bg-base-200 text-gray-400 dark:text-base-content/50'
                        }`}>
                          <item.icon size={24} />
                        </div>
                        <span className={`font-medium text-sm ${isVisible ? 'text-blue-900 dark:text-blue-100' : 'text-gray-500'}`}>
                          {t(item.labelKey, item.fallback)}
                        </span>
                      </div>
                    );
                  })}
                </div>
              </div>
            </div>
          )}

          {/* ── Account ── */}
          {activeTab === 'proxy' && (
            <ProxyTab config={config} saveConfig={saveConfig} t={t} />
          )}

          {activeTab === 'account' && (
            <AccountTab authStatus={authStatus} onLogout={logout} t={t} />
          )}

          {/* ── About ── */}
          {activeTab === 'about' && (
            <div className="flex flex-col" style={{ minHeight: 'calc(100vh - 230px)' }}>
              <div className="flex-1 flex flex-col justify-center items-center space-y-8">
                <div className="text-center space-y-4">
                  <div className="relative inline-block group">
                    <div className="absolute inset-0 bg-orange-500/20 rounded-3xl blur-xl group-hover:blur-2xl transition-all duration-500" />
                    <img
                      src="/icon.png"
                      alt="Logo"
                      className="relative w-24 h-24 rounded-3xl shadow-2xl transform group-hover:scale-105 transition-all duration-500 rotate-3 group-hover:rotate-6 object-cover bg-white dark:bg-black"
                    />
                  </div>
                  <div>
                    <h3 className="text-3xl font-black text-gray-900 dark:text-base-content tracking-tight mb-2">
                      {t('common.app_name', 'Claude Ultra')}
                    </h3>
                    <div className="flex items-center justify-center gap-2 text-sm">
                      v{__APP_VERSION__}
                      <span className="text-gray-400 dark:text-gray-600">&bull;</span>
                      <span className="text-gray-500 dark:text-gray-400">
                        {t('settings.branding.subtitle', 'Multi-Account Manager')}
                      </span>
                    </div>
                  </div>
                </div>

                <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-4 w-full max-w-6xl px-4">
                  <div className="bg-white dark:bg-base-100 p-4 rounded-2xl border border-gray-100 dark:border-base-300 shadow-sm hover:shadow-md hover:border-blue-200 dark:hover:border-blue-800 transition-all group flex flex-col items-center text-center gap-3">
                    <div className="p-3 bg-blue-50 dark:bg-blue-900/20 rounded-xl group-hover:scale-110 transition-transform duration-300">
                      <User className="w-6 h-6 text-blue-500" />
                    </div>
                    <div>
                      <div className="text-xs text-gray-400 uppercase tracking-wider font-semibold mb-1">{t('settings.about.author', 'Author')}</div>
                      <div className="font-bold text-gray-900 dark:text-base-content">Hata</div>
                    </div>
                  </div>
                  <div className="bg-white dark:bg-base-100 p-4 rounded-2xl border border-gray-100 dark:border-base-300 shadow-sm hover:shadow-md hover:border-green-200 dark:hover:border-green-800 transition-all group flex flex-col items-center text-center gap-3">
                    <div className="p-3 bg-green-50 dark:bg-green-900/20 rounded-xl group-hover:scale-110 transition-transform duration-300">
                      <MessageCircle className="w-6 h-6 text-green-500" />
                    </div>
                    <div>
                      <div className="text-xs text-gray-400 uppercase tracking-wider font-semibold mb-1">{t('settings.about.wechat', 'WeChat')}</div>
                      <div className="font-bold text-gray-900 dark:text-base-content">—</div>
                    </div>
                  </div>
                  <div className="bg-white dark:bg-base-100 p-4 rounded-2xl border border-gray-100 dark:border-base-300 shadow-sm hover:shadow-md hover:border-sky-200 dark:hover:border-sky-800 transition-all group flex flex-col items-center text-center gap-3 cursor-pointer">
                    <div className="p-3 bg-sky-50 dark:bg-sky-900/20 rounded-xl group-hover:scale-110 transition-transform duration-300">
                      <Send className="w-6 h-6 text-sky-500" />
                    </div>
                    <div>
                      <div className="text-xs text-gray-400 uppercase tracking-wider font-semibold mb-1">{t('settings.about.telegram', 'Telegram')}</div>
                      <div className="font-bold text-gray-900 dark:text-base-content">—</div>
                    </div>
                  </div>
                  <a href="https://github.com/hatawong/claude-ultra" target="_blank" rel="noreferrer"
                    className="bg-white dark:bg-base-100 p-4 rounded-2xl border border-gray-100 dark:border-base-300 shadow-sm hover:shadow-md hover:border-gray-300 dark:hover:border-gray-600 transition-all group flex flex-col items-center text-center gap-3 cursor-pointer">
                    <div className="p-3 bg-gray-50 dark:bg-gray-800 rounded-xl group-hover:scale-110 transition-transform duration-300">
                      <Github className="w-6 h-6 text-gray-900 dark:text-white" />
                    </div>
                    <div>
                      <div className="text-xs text-gray-400 uppercase tracking-wider font-semibold mb-1">{t('settings.about.github', 'GitHub')}</div>
                      <div className="flex items-center gap-1 font-bold text-gray-900 dark:text-base-content">
                        <span>{t('settings.about.view_code', 'View Code')}</span>
                        <ExternalLink className="w-3 h-3 text-gray-400" />
                      </div>
                    </div>
                  </a>
                  <div className="bg-white dark:bg-base-100 p-4 rounded-2xl border border-gray-100 dark:border-base-300 shadow-sm hover:shadow-md hover:border-pink-200 dark:hover:border-pink-800 transition-all group flex flex-col items-center text-center gap-3 cursor-pointer">
                    <div className="p-3 bg-pink-50 dark:bg-pink-900/20 rounded-xl group-hover:scale-110 transition-transform duration-300">
                      <Heart className="w-6 h-6 text-pink-500 fill-pink-500" />
                    </div>
                    <div>
                      <div className="text-xs text-gray-400 uppercase tracking-wider font-semibold mb-1">{t('settings.about.support_title', 'Support')}</div>
                      <div className="font-bold text-gray-900 dark:text-base-content">{t('settings.about.support_btn', 'Sponsor')}</div>
                    </div>
                  </div>
                </div>

                <div className="flex items-center justify-center gap-3">
                  <div className="flex gap-2">
                    <div className="px-3 py-1 bg-gray-50 dark:bg-base-200 rounded-lg text-xs font-medium text-gray-500 dark:text-gray-400 border border-gray-100 dark:border-base-300">Tauri v2</div>
                    <div className="px-3 py-1 bg-gray-50 dark:bg-base-200 rounded-lg text-xs font-medium text-gray-500 dark:text-gray-400 border border-gray-100 dark:border-base-300">React 19</div>
                    <div className="px-3 py-1 bg-gray-50 dark:bg-base-200 rounded-lg text-xs font-medium text-gray-500 dark:text-gray-400 border border-gray-100 dark:border-base-300">TypeScript</div>
                  </div>
                  <button
                    onClick={checkForUpdates}
                    disabled={isCheckingUpdate}
                    className="px-3 py-1 bg-blue-500 hover:bg-blue-600 disabled:bg-gray-300 dark:disabled:bg-gray-700 text-white text-xs rounded-lg transition-all flex items-center gap-1.5 shadow-sm hover:shadow-md disabled:cursor-not-allowed"
                  >
                    <RefreshCw className={`w-3.5 h-3.5 ${isCheckingUpdate ? 'animate-spin' : ''}`} />
                    {isCheckingUpdate ? t('settings.about.checking_update', 'Checking...') : t('settings.about.check_update', 'Check for Updates')}
                  </button>
                </div>
              </div>
              <div className="flex-1" />
              <div className="text-center text-[10px] text-gray-300 dark:text-gray-600 pb-2">
                {t('settings.about.copyright', '© 2026 Claude Ultra. All rights reserved.')}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ── Account Tab ─────────────────────────────────────────────────────

// ── Proxy Tab ───────────────────────────────────────────────────────

function ProxyTab({
  config,
  saveConfig,
  t,
}: {
  config: AppConfig | null;
  saveConfig: (config: AppConfig, silent?: boolean) => Promise<void>;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  const [proxyForm, setProxyForm] = useState({
    host: config?.proxy?.residential?.host || 'geo.iproyal.com',
    port: config?.proxy?.residential?.port || 12321,
    username: config?.proxy?.residential?.username || '',
    password: config?.proxy?.residential?.password || '',
    default_country: config?.proxy?.residential?.default_country || 'us',
  });
  const [testResult, setTestResult] = useState<{ ok: boolean; mode: string; ip?: string; country?: string; error?: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  const isConfigured = proxyForm.username.trim() !== '' && proxyForm.password.trim() !== '';

  const handleSave = async () => {
    if (!config) return;
    setSaving(true);
    try {
      const newConfig = {
        ...config,
        proxy: {
          ...config.proxy,
          residential: {
            host: proxyForm.host,
            port: proxyForm.port,
            username: proxyForm.username,
            password: proxyForm.password,
            default_country: proxyForm.default_country,
          },
        },
      };
      await saveConfig(newConfig);
      setSaved(true);
      setTimeout(() => setSaved(false), 5000);
    } catch (e) {
      console.error('Failed to save proxy config:', e);
    } finally {
      setSaving(false);
    }
  };

  const saveQuiet = async () => {
    if (!config) return;
    const newConfig = {
      ...config,
      proxy: {
        ...config.proxy,
        residential: {
          host: proxyForm.host,
          port: proxyForm.port,
          username: proxyForm.username,
          password: proxyForm.password,
          default_country: proxyForm.default_country,
        },
      },
    };
    await saveConfig(newConfig);
  };

  const handleTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      // Save current form values silently so backend tests the latest config
      await saveQuiet();
      const result = await invoke<{ ok: boolean; mode: string; ip?: string; country?: string; error?: string }>('test_proxy_connection');
      setTestResult(result);
    } catch (e: any) {
      setTestResult({ ok: false, mode: 'unknown', error: typeof e === 'string' ? e : e.message });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-lg font-semibold text-gray-900 dark:text-base-content">
        {t('proxy.title')}
      </h2>

      {/* Mode indicator */}
      <div className={`flex items-center gap-2 px-4 py-2 rounded-lg text-sm ${
        isConfigured
          ? 'bg-green-50 dark:bg-green-900/20 text-green-700 dark:text-green-400'
          : 'bg-yellow-50 dark:bg-yellow-900/20 text-yellow-700 dark:text-yellow-400'
      }`}>
        <span className={`w-2 h-2 rounded-full ${isConfigured ? 'bg-green-500' : 'bg-yellow-500'}`} />
        {isConfigured ? t('proxy.mode_proxied') : t('proxy.mode_direct')}
      </div>

      {/* Warning for direct mode */}
      {!isConfigured && (
        <div className="bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-lg p-4 text-sm text-red-700 dark:text-red-400">
          {t('proxy.direct_warning')}
        </div>
      )}

      {/* Residential Proxy form */}
      <div className="bg-gray-50 dark:bg-base-200 rounded-xl p-5 border border-gray-100 dark:border-base-300">
        <h3 className="font-medium text-gray-900 dark:text-base-content mb-4">
          {t('proxy.residential_title')}
        </h3>
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="block text-xs text-gray-500 dark:text-gray-400 mb-1">{t('proxy.host')}</label>
            <input
              type="text"
              value={proxyForm.host}
              onChange={(e) => setProxyForm({ ...proxyForm, host: e.target.value })}
              className="w-full px-3 py-1.5 bg-white dark:bg-base-100 text-xs border border-gray-200 dark:border-gray-600 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div>
            <label className="block text-xs text-gray-500 dark:text-gray-400 mb-1">{t('proxy.port')}</label>
            <input
              type="number"
              value={proxyForm.port}
              onChange={(e) => setProxyForm({ ...proxyForm, port: parseInt(e.target.value) || 0 })}
              className="w-full px-3 py-1.5 bg-white dark:bg-base-100 text-xs border border-gray-200 dark:border-gray-600 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div>
            <label className="block text-xs text-gray-500 dark:text-gray-400 mb-1">{t('proxy.username')}</label>
            <input
              type="text"
              value={proxyForm.username}
              onChange={(e) => setProxyForm({ ...proxyForm, username: e.target.value })}
              className="w-full px-3 py-1.5 bg-white dark:bg-base-100 text-xs border border-gray-200 dark:border-gray-600 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div>
            <label className="block text-xs text-gray-500 dark:text-gray-400 mb-1">{t('proxy.password')}</label>
            <input
              type="password"
              value={proxyForm.password}
              onChange={(e) => setProxyForm({ ...proxyForm, password: e.target.value })}
              className="w-full px-3 py-1.5 bg-white dark:bg-base-100 text-xs border border-gray-200 dark:border-gray-600 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div>
            <label className="block text-xs text-gray-500 dark:text-gray-400 mb-1">{t('proxy.country')}</label>
            <input
              type="text"
              value={proxyForm.default_country}
              onChange={(e) => setProxyForm({ ...proxyForm, default_country: e.target.value })}
              className="w-full px-3 py-1.5 bg-white dark:bg-base-100 text-xs border border-gray-200 dark:border-gray-600 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
        </div>
      </div>

      {/* Saved notice */}
      {saved && (
        <div className="px-4 py-2 rounded-lg text-sm bg-blue-50 dark:bg-blue-900/20 text-blue-700 dark:text-blue-400">
          {t('proxy.save_restart')}
        </div>
      )}

      {/* Buttons + test result inline */}
      <div className="flex items-center gap-2">
        {/* Test result (inline) */}
        {testResult && (
          <span className={`text-xs ${
            testResult.ok ? 'text-green-600 dark:text-green-400' : 'text-red-600 dark:text-red-400'
          }`}>
            {testResult.ok
              ? (testResult.mode === 'proxied'
                  ? t('proxy.test_proxied', { ip: testResult.ip, country: testResult.country })
                  : t('proxy.test_direct', { ip: testResult.ip, country: testResult.country }))
              : t('proxy.test_failed', { error: testResult.error })
            }
          </span>
        )}
        <div className="flex-1" />
        <button
          onClick={handleTest}
          disabled={testing}
          className="px-3 py-1.5 text-xs font-medium text-gray-700 dark:text-gray-300 bg-gray-100 dark:bg-base-200 rounded-lg hover:bg-gray-200 dark:hover:bg-base-100 transition-colors disabled:opacity-50"
        >
          {testing ? t('common.loading') : t('proxy.test_connection')}
        </button>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-3 py-1.5 text-xs font-medium text-white bg-blue-500 rounded-lg hover:bg-blue-600 transition-colors disabled:opacity-50"
        >
          {saving ? t('common.loading') : t('proxy.save')}
        </button>
      </div>
    </div>
  );
}

// ── Account Tab ─────────────────────────────────────────────────────

function AccountTab({
  authStatus,
  onLogout,
  t,
}: {
  authStatus: import('../types/auth').AuthStatus | null;
  onLogout: () => Promise<void>;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  const [activationCode, setActivationCode] = useState('');
  const [isActivating, setIsActivating] = useState(false);
  const [isLoggingOut, setIsLoggingOut] = useState(false);
  const { fetchAuthStatus } = useAuthStore();

  // Internal mode
  if (authStatus?.mode === 'internal') {
    return (
      <div className="space-y-4">
        <h2 className="text-lg font-semibold text-gray-900 dark:text-base-content">
          {t('settings.tabs.account', 'Account')}
        </h2>
        <div className="text-center py-12 text-gray-400 dark:text-gray-500">
          {t('auth.internal_mode', 'Internal Mode — No login required')}
        </div>
      </div>
    );
  }

  const user = authStatus?.user;

  const handleActivate = async () => {
    if (!activationCode.trim()) return;
    setIsActivating(true);
    try {
      await invoke<AuthUser>('activate_subscription', { code: activationCode.trim() });
      showToast(t('auth.activation_success', 'Subscription activated!'));
      setActivationCode('');
      await fetchAuthStatus();
    } catch (e: any) {
      showToast(typeof e === 'string' ? e : e.message || 'Activation failed', 'error');
    } finally {
      setIsActivating(false);
    }
  };

  const handleLogout = async () => {
    setIsLoggingOut(true);
    try {
      await onLogout();
    } catch (e) {
      console.error('Logout failed:', e);
      setIsLoggingOut(false);
    }
  };

  const planColor = user?.plan === 'ultra' ? 'amber' : user?.plan === 'max' ? 'purple' : user?.plan === 'pro' ? 'blue' : 'gray';

  return (
    <div className="space-y-6">
      <h2 className="text-lg font-semibold text-gray-900 dark:text-base-content">
        {t('settings.tabs.account', 'Account')}
      </h2>

      {/* Profile card */}
      {user && (
        <div className="bg-gray-50 dark:bg-base-200 rounded-xl p-5 border border-gray-100 dark:border-base-300">
          <div className="flex items-center gap-4">
            <div className="w-12 h-12 rounded-full bg-gray-200 dark:bg-base-300 flex items-center justify-center">
              <User className="w-6 h-6 text-gray-500 dark:text-gray-400" />
            </div>
            <div className="flex-1">
              <div className="font-semibold text-gray-900 dark:text-base-content text-lg">
                {user.displayName}
              </div>
              <div className="flex items-center gap-2 mt-1">
                <span className={`text-xs font-bold px-2 py-0.5 rounded-full ${
                  planColor === 'amber' ? 'bg-amber-100 dark:bg-amber-900/30 text-amber-600 dark:text-amber-400' :
                  planColor === 'purple' ? 'bg-purple-100 dark:bg-purple-900/30 text-purple-600 dark:text-purple-400' :
                  planColor === 'blue' ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400' :
                  'bg-gray-200 dark:bg-gray-700 text-gray-500 dark:text-gray-400'
                }`}>
                  {user.plan.toUpperCase()}
                </span>
                <span className="text-sm text-gray-500 dark:text-gray-400">
                  {t('auth.max_accounts', 'Max {{count}} accounts', { count: user.maxAccounts })}
                </span>
              </div>
              {user.planExpiresAt && (
                <div className="text-xs text-gray-400 dark:text-gray-500 mt-1">
                  {t('auth.expires', 'Expires')}: {new Date(user.planExpiresAt).toLocaleDateString()}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Subscription activation */}
      <div className="bg-gray-50 dark:bg-base-200 rounded-xl p-5 border border-gray-100 dark:border-base-300">
        <h3 className="font-medium text-gray-900 dark:text-base-content mb-3">
          {t('auth.activate_title', 'Activate Subscription')}
        </h3>
        <div className="flex gap-2 items-center">
          <input
            type="text"
            value={activationCode}
            onChange={(e) => setActivationCode(e.target.value)}
            placeholder={t('auth.activation_placeholder', 'Enter activation code (CU-ACT-xxx)')}
            className="flex-1 px-3 py-1.5 bg-white dark:bg-base-200 text-xs text-gray-900 dark:text-base-content border border-gray-200 dark:border-gray-600 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 placeholder:text-gray-400 dark:placeholder:text-gray-500"
          />
          <button
            onClick={handleActivate}
            disabled={isActivating || !activationCode.trim()}
            className="px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex items-center gap-1.5 shrink-0"
          >
            <Key className="w-3.5 h-3.5" />
            {isActivating ? t('common.loading', 'Loading...') : t('auth.activate', 'Activate')}
          </button>
        </div>
      </div>

      {/* Logout */}
      <div className="flex justify-end">
        <button
          onClick={handleLogout}
          disabled={isLoggingOut}
          className="px-4 py-2 text-sm text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/20 rounded-lg transition-colors flex items-center gap-1.5"
        >
          <LogOut className="w-3.5 h-3.5" />
          {isLoggingOut ? t('common.loading', 'Loading...') : t('auth.logout', 'Logout')}
        </button>
      </div>
    </div>
  );
}

export default Settings;
