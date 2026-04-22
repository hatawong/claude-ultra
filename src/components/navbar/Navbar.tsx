import { useState, useRef, useEffect, useCallback } from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  LayoutDashboard, Users, Network, Activity,
  BarChart3, Settings, Lock,
  Sun, Moon,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useConfigStore } from '../../stores/useConfigStore';
import { useAuthStore } from '../../stores/useAuthStore';
import LogoIcon from '/icon.png';

// ── Types & Constants ────────────────────────────────────────────────

interface NavItem {
  path: string;
  labelKey: string;
  fallback: string;
  icon: LucideIcon;
}

const navItems: NavItem[] = [
  { path: '/', labelKey: 'nav.dashboard', fallback: 'Dashboard', icon: LayoutDashboard },
  { path: '/accounts', labelKey: 'nav.accounts', fallback: 'Accounts', icon: Users },
  { path: '/gateway', labelKey: 'nav.gateway', fallback: 'Gateway', icon: Network },
  { path: '/monitor', labelKey: 'nav.call_records', fallback: 'Monitor', icon: Activity },
  { path: '/token-stats', labelKey: 'nav.token_stats', fallback: 'Token Stats', icon: BarChart3 },
  { path: '/security', labelKey: 'nav.security', fallback: 'Security', icon: Lock },
  { path: '/settings', labelKey: 'nav.settings', fallback: 'Settings', icon: Settings },
];

interface Language {
  code: string;
  label: string;
  short: string;
}

const LANGUAGES: Language[] = [
  { code: 'zh', label: '简体中文', short: 'ZH' },
  { code: 'zh-TW', label: '繁體中文', short: 'TW' },
  { code: 'en', label: 'English', short: 'EN' },
  { code: 'ja', label: '日本語', short: 'JA' },
  { code: 'tr', label: 'Türkçe', short: 'TR' },
  { code: 'vi', label: 'Tiếng Việt', short: 'VI' },
  { code: 'pt', label: 'Português', short: 'PT' },
  { code: 'ko', label: '한국어', short: 'KO' },
  { code: 'ru', label: 'Русский', short: 'RU' },
  { code: 'ar', label: 'العربية', short: 'AR' },
  { code: 'es', label: 'Español', short: 'ES' },
  { code: 'my', label: 'Bahasa Melayu', short: 'MY' },
];

function isActive(pathname: string, itemPath: string): boolean {
  if (itemPath === '/') return pathname === '/';
  return pathname.startsWith(itemPath);
}

function useClickOutside(ref: React.RefObject<HTMLElement | null>, handler: () => void) {
  useEffect(() => {
    const listener = (e: MouseEvent) => {
      if (!ref.current || ref.current.contains(e.target as Node)) return;
      handler();
    };
    document.addEventListener('mousedown', listener);
    return () => document.removeEventListener('mousedown', listener);
  }, [ref, handler]);
}

// ── Navbar ───────────────────────────────────────────────────────────

function Navbar() {
  const location = useLocation();
  const navigate = useNavigate();
  const { t, i18n } = useTranslation();
  const { isMenuItemHidden } = useConfigStore();
  const { authStatus } = useAuthStore();

  // Filter out hidden menu items (same pattern as AM NavMenu)
  const visibleNavItems = navItems.filter(item => !isMenuItemHidden(item.path));

  // Dynamic text/icon mode — measure if text capsule fits
  const [useIconMode, setUseIconMode] = useState(false);
  const navContainerRef = useRef<HTMLDivElement>(null);
  const textCapsuleRef = useRef<HTMLElement>(null);

  const checkOverflow = useCallback(() => {
    if (!navContainerRef.current || !textCapsuleRef.current) return;
    const containerWidth = navContainerRef.current.offsetWidth;
    const capsuleWidth = textCapsuleRef.current.scrollWidth;
    // Switch to icon mode if text capsule would overflow (with some margin for logo + settings)
    setUseIconMode(capsuleWidth > containerWidth - 40);
  }, []);

  useEffect(() => {
    checkOverflow();
    const observer = new ResizeObserver(checkOverflow);
    if (navContainerRef.current) observer.observe(navContainerRef.current);
    return () => observer.disconnect();
  }, [checkOverflow, i18n.language, visibleNavItems.length]);

  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    const saved = localStorage.getItem('app-theme-preference');
    return saved === 'light' ? 'light' : 'dark';
  });

  // Sync native UI theme on mount
  useEffect(() => {
    invoke('set_window_theme', { theme }).catch(() => {});
  }, []);

  const toggleTheme = () => {
    const next = theme === 'dark' ? 'light' : 'dark';
    setTheme(next);
    localStorage.setItem('app-theme-preference', next);
    const root = document.documentElement;
    if (next === 'dark') {
      root.classList.add('dark');
      root.setAttribute('data-theme', 'dark');
      root.style.backgroundColor = '#15191e';
    } else {
      root.classList.remove('dark');
      root.setAttribute('data-theme', 'light');
      root.style.backgroundColor = '#FAFBFC';
    }
    // Sync native UI theme (context menus, etc.)
    invoke('set_window_theme', { theme: next }).catch(() => {});
    // Persist to config store (keeps Settings page in sync)
    useConfigStore.getState().updateTheme(next).catch(() => {});
  };

  const handleLanguageChange = async (langCode: string) => {
    i18n.changeLanguage(langCode);
    // RTL support
    document.documentElement.dir = langCode === 'ar' ? 'rtl' : 'ltr';
    // Persist to config store (keeps Settings page in sync)
    useConfigStore.getState().updateLanguage(langCode).catch(() => {});
  };

  return (
    <nav
      style={{ position: 'sticky', top: 0, zIndex: 50 }}
      className="pt-9 transition-all duration-200 bg-[#FAFBFC] dark:bg-base-300"
    >
      {/* Drag region */}
      <div
        className="absolute top-9 left-0 right-0 h-16"
        style={{ zIndex: 5, backgroundColor: 'rgba(0,0,0,0.001)' }}
        data-tauri-drag-region
      />

      <div className="max-w-7xl mx-auto px-8 relative" style={{ zIndex: 10 }}>
        <div className="flex items-center h-16 gap-4">
          {/* Logo */}
          <Link
            to="/"
            draggable="false"
            className="flex items-center gap-2 text-xl font-semibold text-gray-900 dark:text-base-content shrink-0"
          >
            <img
              src={LogoIcon}
              alt="Logo"
              className="w-8 h-8 cursor-pointer active:scale-95 transition-transform rounded-lg ring-1 ring-white/10 shadow-[0_0_8px_rgba(249,115,22,0.3)]"
              draggable="false"
            />
            <span className="hidden min-[880px]:inline text-nowrap">
              {t('common.app_name', 'Claude Ultra')}
            </span>
          </Link>

          {/* Nav capsule — centered */}
          <div className="flex-1 flex justify-center" ref={navContainerRef}>
            {/* Text capsule — invisible when overflows (stays in DOM for measurement) */}
            <nav
              ref={textCapsuleRef as any}
              className="flex items-center gap-1 bg-gray-100 dark:bg-base-200 rounded-full p-1"
              style={useIconMode ? { visibility: 'hidden', position: 'absolute', pointerEvents: 'none' } : undefined}
            >
              {visibleNavItems.map((item) => (
                <Link
                  key={item.path}
                  to={item.path}
                  draggable="false"
                  className={`
                    px-4 xl:px-5 py-2 rounded-full text-sm font-medium transition-all whitespace-nowrap
                    ${isActive(location.pathname, item.path)
                      ? 'bg-gray-900 text-white shadow-sm dark:bg-white dark:text-gray-900'
                      : 'text-gray-700 hover:text-gray-900 hover:bg-gray-200 dark:text-gray-400 dark:hover:text-base-content dark:hover:bg-base-100'
                    }
                  `}
                >
                  {t(item.labelKey, item.fallback)}
                </Link>
              ))}
            </nav>

            {/* Icon capsule — shown when text overflows */}
            <nav className={`${useIconMode ? 'flex' : 'hidden'} items-center gap-1 bg-gray-100 dark:bg-base-200 rounded-full p-1`}>
              {visibleNavItems.map((item) => (
                <Link
                  key={item.path}
                  to={item.path}
                  draggable="false"
                  className={`
                    p-2 rounded-full transition-all
                    ${isActive(location.pathname, item.path)
                      ? 'bg-gray-900 text-white shadow-sm dark:bg-white dark:text-gray-900'
                      : 'text-gray-700 hover:text-gray-900 hover:bg-gray-200 dark:text-gray-400 dark:hover:text-base-content dark:hover:bg-base-100'
                    }
                  `}
                  title={t(item.labelKey, item.fallback)}
                >
                  <item.icon className="w-5 h-5" />
                </Link>
              ))}
            </nav>
          </div>

          {/* User badge (distribution mode only) */}
          {authStatus?.mode === 'distribution' && authStatus.status === 'logged_in' && authStatus.user && (
            <div
              onClick={() => {
                if (location.pathname === '/settings') {
                  window.dispatchEvent(new CustomEvent('settings-tab', { detail: 'account' }));
                } else {
                  navigate('/settings?tab=account');
                }
              }}
              className="flex items-center gap-2 px-3 py-1.5 rounded-full bg-gray-100 dark:bg-base-200 hover:bg-gray-200 dark:hover:bg-base-100 transition-colors shrink-0 cursor-pointer"
            >
              <span className="text-sm font-medium text-gray-700 dark:text-gray-300 max-w-[100px] truncate">
                {authStatus.user.displayName}
              </span>
              <span className={`text-[10px] font-bold px-1.5 py-0.5 rounded-full ${
                authStatus.user.plan === 'ultra' ? 'bg-amber-100 dark:bg-amber-900/30 text-amber-600 dark:text-amber-400' :
                authStatus.user.plan === 'max' ? 'bg-purple-100 dark:bg-purple-900/30 text-purple-600 dark:text-purple-400' :
                authStatus.user.plan === 'pro' ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400' :
                'bg-gray-200 dark:bg-gray-700 text-gray-500 dark:text-gray-400'
              }`}>
                {authStatus.user.plan.toUpperCase()}
              </span>
            </div>
          )}

          {/* Right settings */}
          <NavSettings
            theme={theme}
            currentLanguage={i18n.language}
            onThemeToggle={toggleTheme}
            onLanguageChange={handleLanguageChange}
          />
        </div>
      </div>
    </nav>
  );
}

// ── Right-side settings ──────────────────────────────────────────────

interface NavSettingsProps {
  theme: 'light' | 'dark';
  currentLanguage: string;
  onThemeToggle: () => void;
  onLanguageChange: (langCode: string) => void;
}

function NavSettings({ theme, currentLanguage, onThemeToggle, onLanguageChange }: NavSettingsProps) {
  const { t } = useTranslation();

  return (
    <div className="flex items-center gap-2 shrink-0">
      <button
        onClick={onThemeToggle}
        className="w-10 h-10 rounded-full bg-gray-100 dark:bg-base-200 hover:bg-gray-200 dark:hover:bg-base-100 flex items-center justify-center transition-colors"
        title={theme === 'dark' ? t('nav.theme_to_light', 'Light mode') : t('nav.theme_to_dark', 'Dark mode')}
      >
        {theme === 'dark' ? (
          <Sun className="w-5 h-5 text-gray-700 dark:text-gray-300" />
        ) : (
          <Moon className="w-5 h-5 text-gray-700 dark:text-gray-300" />
        )}
      </button>

      <LanguageSelector currentLanguage={currentLanguage} onLanguageChange={onLanguageChange} />
    </div>
  );
}

// ── Language dropdown ─────────────────────────────────────────────────

interface LanguageSelectorProps {
  currentLanguage: string;
  onLanguageChange: (langCode: string) => void;
}

function LanguageSelector({ currentLanguage, onLanguageChange }: LanguageSelectorProps) {
  const [isOpen, setIsOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const { t } = useTranslation();

  useClickOutside(menuRef, () => setIsOpen(false));

  // Resolve display language (handle variants like zh-CN → zh)
  const resolvedLang = LANGUAGES.find((l) => l.code === currentLanguage)
    || LANGUAGES.find((l) => currentLanguage.startsWith(l.code))
    || LANGUAGES.find((l) => l.code === 'en')!;

  return (
    <div className="relative" ref={menuRef}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="w-10 h-10 rounded-full bg-gray-100 dark:bg-base-200 hover:bg-gray-200 dark:hover:bg-base-100 flex items-center justify-center transition-colors"
        title={t('settings.general.language', 'Language')}
      >
        <span className="text-sm font-bold text-gray-700 dark:text-gray-300">
          {resolvedLang.short}
        </span>
      </button>

      {isOpen && (
        <div className="absolute right-0 mt-2 w-32 bg-white dark:bg-base-200 rounded-xl shadow-lg border border-gray-100 dark:border-base-100 py-1 overflow-hidden">
          {LANGUAGES.map((lang) => (
            <button
              key={lang.code}
              onClick={() => {
                onLanguageChange(lang.code);
                setIsOpen(false);
              }}
              className={`w-full px-4 py-2 text-left text-sm flex items-center justify-between hover:bg-gray-50 dark:hover:bg-base-100 transition-colors ${
                resolvedLang.code === lang.code
                  ? 'text-blue-500 font-medium bg-blue-50 dark:bg-blue-900/10'
                  : 'text-gray-700 dark:text-gray-300'
              }`}
            >
              <div className="flex items-center gap-3">
                <span className="font-mono font-bold w-6">{lang.short}</span>
                <span className="text-xs opacity-70">{lang.label}</span>
              </div>
              {resolvedLang.code === lang.code && (
                <span className="w-1.5 h-1.5 rounded-full bg-blue-500" />
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default Navbar;
