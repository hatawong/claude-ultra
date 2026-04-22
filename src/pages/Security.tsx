import { useState, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Shield, Lock, FileText, Settings, Activity, RefreshCw, Save } from 'lucide-react';
import { IpAccessLogs } from '../components/security/IpAccessLogs';
import { BlacklistManager } from '../components/security/BlacklistManager';
import { WhitelistManager } from '../components/security/WhitelistManager';
import { SecurityConfigPanel } from '../components/security/SecurityConfigPanel';
import { IpStatistics } from '../components/security/IpStatistics';

const Security: React.FC = () => {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<'logs' | 'stats' | 'blacklist' | 'whitelist' | 'config'>('logs');
  const [refreshKey, setRefreshKey] = useState(0);
  const configSaveRef = useRef<(() => void) | null>(null);

  const handleRefresh = () => setRefreshKey(prev => prev + 1);

  const renderContent = () => {
    switch (activeTab) {
      case 'logs': return <IpAccessLogs refreshKey={refreshKey} />;
      case 'stats': return <IpStatistics refreshKey={refreshKey} />;
      case 'blacklist': return <BlacklistManager refreshKey={refreshKey} />;
      case 'whitelist': return <WhitelistManager refreshKey={refreshKey} />;
      case 'config': return <SecurityConfigPanel onSaveRef={configSaveRef} />;
      default: return <IpAccessLogs refreshKey={refreshKey} />;
    }
  };

  const tabs = [
    { id: 'logs' as const, label: t('security.tabs.logs', 'Access Logs'), icon: FileText },
    { id: 'stats' as const, label: t('security.tabs.stats', 'IP Statistics'), icon: Activity },
    { id: 'blacklist' as const, label: t('security.tabs.blacklist', 'Blacklist'), icon: Shield },
    { id: 'whitelist' as const, label: t('security.tabs.whitelist', 'Whitelist'), icon: Lock },
    { id: 'config' as const, label: t('security.tabs.config', 'Config'), icon: Settings },
  ];

  return (
    <div className="h-full flex flex-col p-5 gap-4 max-w-7xl mx-auto w-full">
      {/* Tab bar + Refresh button */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-0.5 bg-gray-100 dark:bg-base-200 rounded-full p-0.5 w-fit">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
              className={`flex items-center gap-1.5 px-4 py-1.5 rounded-full text-xs font-medium transition-all ${
                activeTab === tab.id
                  ? 'bg-gray-200 dark:bg-gray-700 text-gray-900 dark:text-gray-100 shadow-sm'
                  : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-200'
              }`}
            >
              <tab.icon size={14} />
              {tab.label}
            </button>
          ))}
        </div>
        {activeTab === 'config' ? (
          <button
            onClick={() => configSaveRef.current?.()}
            className="px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1.5 shadow-sm"
          >
            <Save size={14} />
            {t('common.save', 'Save')}
          </button>
        ) : (
          <button
            onClick={handleRefresh}
            className="px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1.5 shadow-sm"
          >
            <RefreshCw size={14} />
            {t('common.refresh', 'Refresh')}
          </button>
        )}
      </div>

      {/* Content */}
      {activeTab === 'logs' ? (
        <IpAccessLogs refreshKey={refreshKey} />
      ) : (
        <div className="flex-1 overflow-hidden flex flex-col bg-white dark:bg-base-100 rounded-lg shadow-sm border border-gray-100 dark:border-base-200">
          {renderContent()}
        </div>
      )}
    </div>
  );
};

export default Security;
