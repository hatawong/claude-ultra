import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Search } from 'lucide-react';
import { getAccessLogs, type AccessLogEntry } from '../../services/securityService';
import Pagination from '../common/Pagination';

interface Props {
  refreshKey?: number;
}

export const IpAccessLogs: React.FC<Props> = ({ refreshKey }) => {
  const { t } = useTranslation();
  const [logs, setLogs] = useState<AccessLogEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(50);
  const [search, setSearch] = useState('');

  const loadLogs = async () => {
    setLoading(true);
    try {
      const res = await getAccessLogs(pageSize, (page - 1) * pageSize, undefined, search || undefined);
      setLogs(res.logs);
      setTotal(res.total);
    } catch (e) {
      console.error('Failed to load logs', e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { loadLogs(); }, [page, pageSize, refreshKey]);

  const handleSearch = () => { setPage(1); loadLogs(); };

  const totalPages = Math.ceil(total / pageSize);

  return (
    <>
    {/* Search — no border */}
    <div className="px-1 py-2 flex items-center gap-2">
      <div className="relative flex-1">
        <Search className="absolute left-2.5 top-2 text-gray-400" size={14} />
        <input
          type="text"
          placeholder={t('security.logs.search_placeholder', 'Search by IP, path, or user agent...')}
          className="w-full pl-9 pr-3 py-1.5 text-xs border border-gray-200 dark:border-gray-600 rounded-lg bg-white dark:bg-base-200 text-gray-900 dark:text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          onBlur={handleSearch}
        />
      </div>
    </div>

    {/* Table — with border */}
    <div className="flex-1 overflow-hidden flex flex-col bg-white dark:bg-base-100 rounded-lg shadow-sm border border-gray-100 dark:border-base-200">
      <div className="flex-1 overflow-auto">
        <table className="w-full text-xs">
          <thead className="bg-gray-50 dark:bg-base-200 text-gray-500 dark:text-gray-400 sticky top-0 z-10">
            <tr>
              <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 60 }}>{t('security.logs.status', 'Status')}</th>
              <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 60 }}>{t('security.logs.method', 'Method')}</th>
              <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 180 }}>{t('security.logs.model', 'Model')}</th>
              <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 120 }}>{t('security.logs.ip', 'IP')}</th>
              <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider">{t('security.logs.path', 'Path')}</th>
              <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 80 }}>{t('security.logs.duration', 'Duration')}</th>
              <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 140 }}>{t('security.logs.time', 'Time')}</th>
            </tr>
          </thead>
          <tbody className="text-gray-700 dark:text-gray-300 divide-y divide-gray-50 dark:divide-base-200">
            {logs.map((log) => (
              <tr key={log.id} className="hover:bg-blue-50 dark:hover:bg-blue-900/20 transition-colors cursor-pointer">
                <td className="py-1.5 px-3">
                  <span className={`px-1.5 py-0.5 rounded text-white text-[10px] font-bold ${
                    log.status >= 200 && log.status < 400 ? 'bg-green-500'
                    : log.status === 403 ? 'bg-red-500'
                    : 'bg-amber-500'
                  }`}>
                    {log.status}
                  </span>
                </td>
                <td className="py-1.5 px-3 font-bold">{log.method}</td>
                <td className="py-1.5 px-3 text-blue-600 dark:text-blue-400 truncate" style={{ maxWidth: 180 }}>{log.model || '\u2014'}</td>
                <td className="py-1.5 px-3">{log.client_ip || '\u2014'}</td>
                <td className="py-1.5 px-3 text-gray-500 truncate text-[10px]" style={{ maxWidth: 200 }} title={log.url}>
                  {log.url}
                </td>
                <td className="py-1.5 px-3 text-right">{(log.duration_ms / 1000).toFixed(1)}s</td>
                <td className="py-1.5 px-3 text-right text-[10px] text-gray-500">
                  {new Date(log.timestamp).toLocaleString()}
                </td>
              </tr>
            ))}
            {!loading && logs.length === 0 && (
              <tr>
                <td colSpan={7} className="text-center py-10 text-gray-400 text-xs">
                  {t('security.logs.no_data', 'No access logs found')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

    </div>
    <div className="flex-none shrink-0">
      <Pagination
        currentPage={page}
        totalPages={totalPages}
        onPageChange={setPage}
        totalItems={total}
        itemsPerPage={pageSize}
        totalOnly={true}
        onPageSizeChange={(s) => { setPageSize(s); setPage(1); }}
      />
    </div>
    </>
  );
};
