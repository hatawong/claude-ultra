import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Trash2, Check, Plus, Search, X, ShieldCheck } from 'lucide-react';
import { listWhitelist, addWhitelist, removeWhitelist, type WhitelistEntry } from '../../services/securityService';

interface Props {
  refreshKey?: number;
}

export const WhitelistManager: React.FC<Props> = ({ refreshKey }) => {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<WhitelistEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState('');
  const [isAddOpen, setIsAddOpen] = useState(false);
  const [newIp, setNewIp] = useState('');
  const [newDescription, setNewDescription] = useState('');

  const load = async () => {
    setLoading(true);
    try {
      setEntries(await listWhitelist());
    } catch (e) {
      console.error('Failed to load whitelist', e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, [refreshKey]);

  const handleAdd = async () => {
    try {
      await addWhitelist(newIp, newDescription || undefined);
      setIsAddOpen(false);
      setNewIp(''); setNewDescription('');
      load();
    } catch (e) {
      console.error('Failed to add to whitelist', e);
      alert(t('security.whitelist.add_failed', 'Failed to add IP') + ': ' + e);
    }
  };

  const handleRemove = async (entry: WhitelistEntry) => {
    setEntries(prev => prev.filter(e => e.id !== entry.id));
    try {
      await removeWhitelist(entry.id, entry.ip);
    } catch (e) {
      console.error('Failed to remove from whitelist', e);
      load();
    }
  };

  const filtered = entries.filter(e =>
    e.ip.includes(search) || (e.description && e.description.toLowerCase().includes(search.toLowerCase()))
  );

  return (
    <div className="flex flex-col h-full">
      <div className="px-3 py-2 border-b border-gray-100 dark:border-base-200 flex items-center gap-2">
        <button
          onClick={() => setIsAddOpen(true)}
          className="px-3 py-1.5 bg-gray-700 dark:bg-base-200 text-white dark:text-gray-300 text-xs font-medium rounded-lg hover:bg-gray-800 dark:hover:bg-base-100 transition-colors flex items-center gap-1.5 shadow-sm"
        >
          <Plus size={14} /> {t('security.whitelist.add_btn', 'Add IP')}
        </button>
        <div className="relative flex-1">
          <Search className="absolute left-2.5 top-2 text-gray-400" size={14} />
          <input
            type="text"
            placeholder={t('security.whitelist.search_placeholder', 'Search IP or description...')}
            className="w-full pl-9 pr-3 py-1.5 text-xs border border-gray-200 dark:border-gray-600 rounded-lg bg-white dark:bg-base-200 text-gray-900 dark:text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
      </div>

      <div className="flex-1 overflow-auto p-4">
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {filtered.map(entry => (
            <div key={entry.id} className="bg-white dark:bg-base-100 border border-green-100 dark:border-green-900/30 rounded-lg p-4 shadow-sm hover:shadow-md transition-shadow relative group">
              <div className="absolute top-0 right-0 p-2 opacity-10">
                <ShieldCheck size={64} className="text-green-500" />
              </div>
              <div className="flex items-start justify-between mb-2 relative z-10">
                <h3 className="font-mono font-bold text-lg text-green-700 dark:text-green-400">{entry.ip}</h3>
                <button
                  onClick={() => handleRemove(entry)}
                  className="p-1 rounded text-red-500 hover:bg-red-500/10 opacity-0 group-hover:opacity-100 transition-all"
                >
                  <Trash2 size={14} />
                </button>
              </div>
              {entry.description && (
                <p className="text-sm text-gray-600 dark:text-gray-400 mb-2 flex items-center gap-1 relative z-10">
                  <Check size={12} className="text-green-500" /> {entry.description}
                </p>
              )}
              <div className="text-xs text-gray-400 mt-3 pt-3 border-t border-gray-50 dark:border-base-200 relative z-10">
                Added: {new Date(entry.created_at * 1000).toLocaleString()}
              </div>
            </div>
          ))}
          {!loading && filtered.length === 0 && (
            <div className="col-span-full text-center py-10 text-gray-400">{t('security.whitelist.no_data', 'No whitelisted IPs')}</div>
          )}
        </div>
      </div>

      {/* Add Modal */}
      {isAddOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setIsAddOpen(false)}>
          <div className="bg-base-100 rounded-lg shadow-2xl w-[420px] flex flex-col border border-base-300" onClick={e => e.stopPropagation()}>
            <div className="flex items-center justify-between px-5 pt-4 pb-3">
              <div className="text-sm font-semibold text-base-content">{t('security.whitelist.add_title', 'Add to Whitelist')}</div>
              <button onClick={() => setIsAddOpen(false)} className="p-1.5 rounded-md text-gray-400 hover:text-gray-300 hover:bg-base-200 transition-colors">
                <X size={16} />
              </button>
            </div>
            <div className="px-5 pb-5 space-y-3">
              <div>
                <label className="block text-xs font-medium text-gray-400 mb-1">{t('security.whitelist.ip_label', 'IP Address')}</label>
                <input type="text" className="w-full px-3 py-2 bg-base-200 text-sm text-base-content border border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent placeholder:text-gray-500" placeholder={t('security.whitelist.ip_placeholder', 'e.g. 192.168.1.100')} value={newIp} onChange={e => setNewIp(e.target.value)} />
              </div>
              <div>
                <label className="block text-xs font-medium text-gray-400 mb-1">{t('security.whitelist.desc_label', 'Description (optional)')}</label>
                <input type="text" className="w-full px-3 py-2 bg-base-200 text-sm text-base-content border border-base-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent placeholder:text-gray-500" placeholder={t('security.whitelist.desc_placeholder', 'e.g. Home Mac')} value={newDescription} onChange={e => setNewDescription(e.target.value)} />
              </div>
              <div className="flex justify-end gap-2 pt-2">
                <button className="px-3 py-1.5 text-xs text-gray-400 hover:text-gray-300 hover:bg-base-200 rounded-lg transition-colors" onClick={() => setIsAddOpen(false)}>{t('common.cancel', 'Cancel')}</button>
                <button className="px-3 py-1.5 text-xs bg-green-500 text-white rounded-lg hover:bg-green-600 transition-colors disabled:opacity-50" onClick={handleAdd} disabled={!newIp}>{t('security.whitelist.allow_btn', 'Allow IP')}</button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
