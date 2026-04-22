import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Activity, Users, ShieldAlert, Globe } from 'lucide-react';
import { getIpStatistics, type IpStatsResponse } from '../../services/securityService';

function formatCompact(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
  return n.toString();
}

interface Props {
  refreshKey?: number;
}

export const IpStatistics: React.FC<Props> = ({ refreshKey }) => {
  const { t } = useTranslation();
  const [stats, setStats] = useState<IpStatsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [timeRange, setTimeRange] = useState(24);

  const loadStats = async () => {
    setLoading(true);
    try {
      const data = await getIpStatistics(timeRange);
      setStats(data);
    } catch (e) {
      console.error('Failed to load stats', e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { loadStats(); }, [timeRange, refreshKey]);

  if (loading && !stats) {
    return <div className="p-10 text-center text-xs text-gray-400">{t('common.loading', 'Loading...')}</div>;
  }
  if (!stats) {
    return <div className="p-10 text-center text-xs text-gray-500">{t('security.stats.no_data', 'No data')}</div>;
  }

  const maxReqCount = Math.max(...stats.top_ips.map(ip => ip.request_count), 1);

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {/* Overview Cards */}
        <div className="grid grid-cols-3 gap-3">
          <div className="bg-gray-100 dark:bg-base-200 rounded-xl p-4 border border-gray-100 dark:border-base-300">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-blue-50 dark:bg-blue-900/20 rounded-md">
                <Activity className="w-4 h-4 text-blue-500 dark:text-blue-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{formatCompact(stats.total_requests)}</div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('security.stats.total_requests', 'Total Requests')}</div>
          </div>
          <div className="bg-gray-100 dark:bg-base-200 rounded-xl p-4 border border-gray-100 dark:border-base-300">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-purple-50 dark:bg-purple-900/20 rounded-md">
                <Users className="w-4 h-4 text-purple-500 dark:text-purple-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{formatCompact(stats.unique_ips)}</div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('security.stats.unique_ips', 'Unique IPs')}</div>
          </div>
          <div className="bg-gray-100 dark:bg-base-200 rounded-xl p-4 border border-gray-100 dark:border-base-300">
            <div className="flex items-center justify-between mb-2">
              <div className="p-1.5 bg-red-50 dark:bg-red-900/20 rounded-md">
                <ShieldAlert className="w-4 h-4 text-red-500 dark:text-red-400" />
              </div>
            </div>
            <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{formatCompact(stats.blocked_requests)}</div>
            <div className="text-xs text-gray-500 dark:text-gray-400">{t('security.stats.blocked', 'Blocked')}</div>
          </div>
        </div>

        {/* IP Ranking Table */}
        <div className="bg-gray-100 dark:bg-base-200 rounded-xl shadow-sm border border-gray-100 dark:border-base-300 overflow-hidden">
          <div className="px-4 pt-4 pb-2 flex items-center justify-between">
            <h2 className="text-base font-semibold text-gray-900 dark:text-base-content flex items-center gap-2">
              <Globe className="w-4 h-4 text-blue-500" />
              {t('security.stats.ip_activity', 'IP Activity & Token Usage')}
            </h2>
            <div className="flex gap-1">
              {[1, 24, 168, 720].map(h => (
                <button
                  key={h}
                  className={`px-2 py-0.5 text-[10px] font-medium rounded transition-colors ${
                    timeRange === h
                      ? 'bg-blue-500 text-white'
                      : 'text-gray-500 hover:text-gray-300 hover:bg-base-300'
                  }`}
                  onClick={() => setTimeRange(h)}
                >
                  {h === 1 ? '1h' : h === 24 ? '24h' : h === 168 ? '7d' : '30d'}
                </button>
              ))}
            </div>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead className="bg-gray-50 dark:bg-base-200 text-gray-500 dark:text-gray-400 sticky top-0 z-10">
                <tr>
                  <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: 40 }}>#</th>
                  <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider">{t('security.stats.ip_address', 'IP Address')}</th>
                  <th className="text-left py-2 px-3 font-medium text-xs uppercase tracking-wider" style={{ width: '25%' }}>{t('security.stats.requests', 'Requests')}</th>
                  <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider">{t('security.stats.total_tokens_col', 'Total Tokens')}</th>
                  <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider text-gray-400">{t('security.stats.input', 'Input')}</th>
                  <th className="text-right py-2 px-3 font-medium text-xs uppercase tracking-wider text-gray-400">{t('security.stats.output', 'Output')}</th>
                </tr>
              </thead>
              <tbody>
                {stats.top_ips.map((ip, i) => {
                  const pct = Math.min(100, (ip.request_count / maxReqCount) * 100);
                  let colorClass = 'text-green-500';
                  if (ip.total_tokens > 1_000_000) colorClass = 'text-red-500 font-bold';
                  else if (ip.total_tokens > 100_000) colorClass = 'text-yellow-500 font-bold';
                  else if (ip.total_tokens > 10_000) colorClass = 'text-blue-500';
                  return (
                    <tr key={ip.client_ip} className="border-b border-gray-50 dark:border-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors">
                      <td className="py-1.5 px-3 font-bold text-gray-400 text-[10px]">#{i + 1}</td>
                      <td className="py-1.5 px-3 text-[10px]">{ip.client_ip}</td>
                      <td className="py-1.5 px-3">
                        <div className="flex flex-col gap-0.5">
                          <div className="flex justify-between text-[10px] text-gray-500">
                            <span>{formatCompact(ip.request_count)} {t('security.stats.reqs_abbr', 'reqs')}</span>
                            <span>{Math.round(pct)}%</span>
                          </div>
                          <div className="w-full bg-gray-100 dark:bg-base-300 rounded-full h-1">
                            <div className="bg-blue-500 h-1 rounded-full transition-all" style={{ width: `${pct}%` }} />
                          </div>
                        </div>
                      </td>
                      <td className={`py-1.5 px-3 text-right text-sm ${colorClass}`}>{formatCompact(ip.total_tokens)}</td>
                      <td className="py-1.5 px-3 text-right text-gray-500 text-[10px]">{formatCompact(ip.input_tokens)}</td>
                      <td className="py-1.5 px-3 text-right text-gray-500 text-[10px]">{formatCompact(ip.output_tokens)}</td>
                    </tr>
                  );
                })}
                {stats.top_ips.length === 0 && (
                  <tr><td colSpan={6} className="text-center py-8 text-gray-500 text-xs">{t('security.stats.no_data', 'No data')}</td></tr>
                )}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  );
};
