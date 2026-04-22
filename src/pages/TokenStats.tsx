import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  BarChart, Bar, XAxis, YAxis, CartesianGrid,
  Tooltip, ResponsiveContainer, PieChart, Pie, Cell,
} from 'recharts';
import { Clock, Calendar, CalendarDays, Zap, TrendingUp, RefreshCw, Cpu, DollarSign } from 'lucide-react';
import * as gatewayService from '../services/gatewayService';
import type { TokenStatsRow, CostSummary } from '../services/gatewayService';

type TimeRange = 'hour' | 'day' | 'week';

const COLORS = ['#3b82f6', '#8b5cf6', '#ec4899', '#f59e0b', '#10b981', '#06b6d4', '#6366f1', '#f43f5e'];

const formatNumber = (num: number): string => {
  if (num >= 1_000_000) return `${(num / 1_000_000).toFixed(1)}M`;
  if (num >= 1_000) return `${(num / 1_000).toFixed(1)}K`;
  return num.toString();
};

const formatCost = (cost: number): string => {
  if (cost >= 1) return `$${cost.toFixed(2)}`;
  if (cost >= 0.01) return `$${cost.toFixed(3)}`;
  return `$${cost.toFixed(6)}`;
};

function TokenStats() {
  const { t } = useTranslation();
  const [timeRange, setTimeRange] = useState<TimeRange>('day');
  const [chartData, setChartData] = useState<TokenStatsRow[]>([]);
  const [costSummary, setCostSummary] = useState<CostSummary | null>(null);
  const [loading, setLoading] = useState(true);

  const fetchData = async () => {
    setLoading(true);
    try {
      const now = Date.now();
      let dateFrom: number;
      switch (timeRange) {
        case 'hour': dateFrom = now - 24 * 3600 * 1000; break;
        case 'day': dateFrom = now - 7 * 24 * 3600 * 1000; break;
        case 'week': dateFrom = now - 30 * 24 * 3600 * 1000; break;
      }

      const [stats, summary] = await Promise.all([
        gatewayService.getTokenStats(timeRange),
        gatewayService.getCostSummary(dateFrom, now),
      ]);

      setChartData(stats);
      setCostSummary(summary);
    } catch (error) {
      console.error('Failed to fetch token stats:', error);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
  }, [timeRange]);

  const pieData = costSummary?.by_model.slice(0, 8).map((m, i) => ({
    name: m.model,
    value: m.total_tokens,
    color: COLORS[i % COLORS.length],
  })) || [];

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="p-5 space-y-4 max-w-7xl mx-auto">
        {/* Header */}
        <div className="flex items-center justify-between">
          <h1 className="text-2xl font-bold text-gray-800 dark:text-white flex items-center gap-2">
            <Zap className="w-6 h-6 text-blue-500" />
            {t('token_stats.title', 'Token Stats')}
          </h1>
          <div className="flex items-center gap-2">
            <div className="flex gap-0.5 bg-gray-100/80 dark:bg-base-200 p-1 rounded-xl border border-gray-200/50 dark:border-white/5">
              {([
                { key: 'hour' as TimeRange, icon: Clock, label: t('token_stats.hourly', 'Hourly') },
                { key: 'day' as TimeRange, icon: Calendar, label: t('token_stats.daily', 'Daily') },
                { key: 'week' as TimeRange, icon: CalendarDays, label: t('token_stats.weekly', 'Weekly') },
              ]).map(({ key, icon: Icon, label }) => (
                <button
                  key={key}
                  onClick={() => setTimeRange(key)}
                  className={`px-2 md:px-3 py-1.5 rounded-lg text-[11px] font-semibold transition-all flex items-center gap-1 md:gap-1.5 whitespace-nowrap ${
                    timeRange === key
                      ? 'bg-white dark:bg-base-100 text-blue-600 dark:text-blue-400 shadow-sm ring-1 ring-black/5'
                      : 'text-gray-500 dark:text-gray-400 hover:text-gray-900 dark:hover:text-base-content hover:bg-white/40'
                  }`}
                >
                  <Icon className="w-3.5 h-3.5" />
                  {label}
                </button>
              ))}
            </div>
            <button
              onClick={fetchData}
              disabled={loading}
              className="p-2 rounded-lg bg-blue-500 text-white hover:bg-blue-600 transition-colors disabled:opacity-50"
            >
              <RefreshCw className={`w-4 h-4 ${loading ? 'animate-spin' : ''}`} />
            </button>
          </div>
        </div>

        {/* Summary cards */}
        {costSummary && (
          <div className="grid grid-cols-2 md:grid-cols-5 gap-4">
            <div className="bg-gradient-to-br from-white to-gray-50 dark:from-gray-800 dark:to-gray-800/50 rounded-xl p-4 shadow-sm border border-gray-200 dark:border-gray-700">
              <div className="flex items-center gap-2 text-gray-500 dark:text-gray-400 text-sm mb-2">
                <Zap className="w-4 h-4" />
                {t('token_stats.total_tokens', 'Total Tokens')}
              </div>
              <div className="text-2xl font-bold text-gray-800 dark:text-white">
                {formatNumber(costSummary.total_tokens)}
              </div>
            </div>
            <div className="bg-gradient-to-br from-blue-50/50 to-white dark:from-blue-900/10 dark:to-gray-800 rounded-xl p-4 shadow-sm border border-blue-100 dark:border-blue-900/30">
              <div className="flex items-center gap-2 text-blue-600/80 dark:text-blue-400/80 text-sm mb-2">
                <TrendingUp className="w-4 h-4" />
                {t('token_stats.input', 'Input')}
              </div>
              <div className="text-2xl font-bold text-blue-600 dark:text-blue-400">
                {formatNumber(costSummary.total_input_tokens)}
              </div>
            </div>
            <div className="bg-gradient-to-br from-purple-50/50 to-white dark:from-purple-900/10 dark:to-gray-800 rounded-xl p-4 shadow-sm border border-purple-100 dark:border-purple-900/30">
              <div className="flex items-center gap-2 text-purple-600/80 dark:text-purple-400/80 text-sm mb-2">
                <TrendingUp className="w-4 h-4 rotate-180" />
                {t('token_stats.output', 'Output')}
              </div>
              <div className="text-2xl font-bold text-purple-600 dark:text-purple-400">
                {formatNumber(costSummary.total_output_tokens)}
              </div>
            </div>
            <div className="bg-gradient-to-br from-green-50/50 to-white dark:from-green-900/10 dark:to-gray-800 rounded-xl p-4 shadow-sm border border-green-100 dark:border-green-900/30">
              <div className="flex items-center gap-2 text-green-600/80 dark:text-green-400/80 text-sm mb-2">
                <DollarSign className="w-4 h-4" />
                {t('token_stats.total_cost', 'Total Cost')}
              </div>
              <div className="text-2xl font-bold text-green-600 dark:text-green-400">
                {formatCost(costSummary.total_cost)}
              </div>
            </div>
            <div className="bg-gradient-to-br from-orange-50/50 to-white dark:from-orange-900/10 dark:to-gray-800 rounded-xl p-4 shadow-sm border border-orange-100 dark:border-orange-900/30">
              <div className="flex items-center gap-2 text-orange-600/80 dark:text-orange-400/80 text-sm mb-2">
                <Cpu className="w-4 h-4" />
                {t('token_stats.requests', 'Requests')}
              </div>
              <div className="text-2xl font-bold text-orange-600 dark:text-orange-400">
                {costSummary.total_requests.toLocaleString()}
              </div>
            </div>
          </div>
        )}

        {/* Token usage trend chart */}
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
          <div className="lg:col-span-2 bg-white dark:bg-gray-800 rounded-xl p-6 shadow-sm border border-gray-200 dark:border-gray-700 flex flex-col">
            <h2 className="text-lg font-semibold text-gray-800 dark:text-white mb-4">
              {t('token_stats.usage_trend', 'Token Usage Trend')}
            </h2>
            <div className="flex-1 min-h-[16rem]">
              {chartData.length > 0 ? (
                <ResponsiveContainer width="100%" height="100%">
                  <BarChart data={chartData}>
                    <CartesianGrid strokeDasharray="3 3" vertical={false} stroke="#374151" strokeOpacity={0.15} />
                    <XAxis
                      dataKey="period"
                      tick={{ fontSize: 11, fill: '#6b7280' }}
                      tickFormatter={(val) => {
                        if (timeRange === 'hour') return val.split(' ')[1] || val;
                        if (timeRange === 'day') return val.split('-').slice(1).join('/');
                        return val;
                      }}
                      axisLine={false}
                      tickLine={false}
                      dy={10}
                    />
                    <YAxis
                      tick={{ fontSize: 11, fill: '#6b7280' }}
                      tickFormatter={formatNumber}
                      axisLine={false}
                      tickLine={false}
                    />
                    <Tooltip
                      contentStyle={{
                        backgroundColor: 'rgba(31, 41, 55, 0.95)',
                        border: '1px solid rgba(55, 65, 81, 0.5)',
                        borderRadius: '12px',
                        fontSize: '12px',
                        color: '#e5e7eb',
                      }}
                      cursor={{ fill: 'transparent' }}
                      formatter={(value) => formatNumber(Number(value))}
                    />
                    <Bar dataKey="total_input_tokens" name={t('token_stats.input', 'Input')} fill="#3b82f6" radius={[4, 4, 0, 0]} maxBarSize={50} />
                    <Bar dataKey="total_output_tokens" name={t('token_stats.output', 'Output')} fill="#8b5cf6" radius={[4, 4, 0, 0]} maxBarSize={50} />
                  </BarChart>
                </ResponsiveContainer>
              ) : (
                <div className="h-full flex items-center justify-center text-gray-400">
                  {loading ? t('common.loading', 'Loading...') : t('token_stats.no_data', 'No data')}
                </div>
              )}
            </div>
          </div>

          {/* Model distribution pie chart */}
          <div className="bg-white dark:bg-gray-800 rounded-xl p-6 shadow-sm border border-gray-200 dark:border-gray-700">
            <h2 className="text-lg font-semibold text-gray-800 dark:text-white mb-4">
              {t('token_stats.by_model', 'By Model')}
            </h2>
            <div className="h-48">
              {pieData.length > 0 ? (
                <ResponsiveContainer width="100%" height="100%">
                  <PieChart>
                    <Pie
                      data={pieData}
                      cx="50%"
                      cy="50%"
                      innerRadius={40}
                      outerRadius={70}
                      paddingAngle={2}
                      dataKey="value"
                    >
                      {pieData.map((entry, index) => (
                        <Cell key={`cell-${index}`} fill={entry.color} />
                      ))}
                    </Pie>
                    <Tooltip
                      contentStyle={{
                        backgroundColor: 'rgba(31, 41, 55, 0.95)',
                        border: '1px solid rgba(55, 65, 81, 0.5)',
                        borderRadius: '12px',
                        fontSize: '12px',
                        color: '#e5e7eb',
                      }}
                      formatter={(value) => formatNumber(Number(value))}
                    />
                  </PieChart>
                </ResponsiveContainer>
              ) : (
                <div className="h-full flex items-center justify-center text-gray-400">
                  {loading ? t('common.loading', 'Loading...') : t('token_stats.no_data', 'No data')}
                </div>
              )}
            </div>
            {/* Model list */}
            <div className="mt-4 space-y-2 max-h-32 overflow-y-auto">
              {costSummary?.by_model.slice(0, 5).map((m, i) => (
                <div key={m.model} className="flex items-center justify-between text-sm">
                  <div className="flex items-center gap-2">
                    <div className="w-3 h-3 rounded-full" style={{ backgroundColor: COLORS[i % COLORS.length] }} />
                    <span className="text-gray-600 dark:text-gray-300 truncate max-w-[120px]">{m.model}</span>
                  </div>
                  <div className="flex items-center gap-3">
                    <span className="font-medium text-gray-800 dark:text-white">{formatNumber(m.total_tokens)}</span>
                    <span className="text-gray-500 text-xs">{formatCost(m.total_cost)}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>

        {/* Model pricing reference */}
        <div className="bg-white dark:bg-gray-800 rounded-xl p-6 shadow-sm border border-gray-200 dark:border-gray-700">
          <h2 className="text-lg font-semibold text-gray-800 dark:text-white mb-4 flex items-center gap-2">
            <DollarSign className="w-5 h-5 text-green-500" />
            {t('token_stats.model_pricing', 'Model Pricing (per MTok)')}
          </h2>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-200 dark:border-gray-700">
                  <th className="text-left py-3 px-4 font-medium text-gray-500 dark:text-gray-400">{t('token_stats.model', 'Model')}</th>
                  <th className="text-right py-3 px-4 font-medium text-gray-500 dark:text-gray-400">{t('token_stats.input', 'Input')}</th>
                  <th className="text-right py-3 px-4 font-medium text-gray-500 dark:text-gray-400">{t('token_stats.output', 'Output')}</th>
                  <th className="text-right py-3 px-4 font-medium text-gray-500 dark:text-gray-400">{t('token_stats.cache_write', 'Cache Write')}</th>
                  <th className="text-right py-3 px-4 font-medium text-gray-500 dark:text-gray-400">{t('token_stats.cache_read', 'Cache Read')}</th>
                </tr>
              </thead>
              <tbody>
                {[
                  { model: 'Claude Opus 4.7', input: '$5', output: '$25', cacheW: '$6.25', cacheR: '$0.50' },
                  { model: 'Claude Opus 4.6', input: '$5', output: '$25', cacheW: '$6.25', cacheR: '$0.50' },
                  { model: 'Claude Sonnet 4.6', input: '$3', output: '$15', cacheW: '$3.75', cacheR: '$0.30' },
                  { model: 'Claude Haiku 4.5', input: '$1', output: '$5', cacheW: '$1.25', cacheR: '$0.10' },
                ].map((row) => (
                  <tr key={row.model} className="border-b border-gray-100 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30">
                    <td className="py-3 px-4 text-gray-800 dark:text-white font-medium">{row.model}</td>
                    <td className="py-3 px-4 text-right text-blue-600">{row.input}</td>
                    <td className="py-3 px-4 text-right text-purple-600">{row.output}</td>
                    <td className="py-3 px-4 text-right text-amber-600">{row.cacheW}</td>
                    <td className="py-3 px-4 text-right text-green-600">{row.cacheR}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  );
}

export default TokenStats;
