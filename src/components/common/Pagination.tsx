import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { cn } from '../../utils/cn';

interface PaginationProps {
  currentPage: number;
  totalPages: number;
  onPageChange: (page: number) => void;
  totalItems: number;
  itemsPerPage: number;
  selectedCount?: number;
  totalOnly?: boolean;
  onPageSizeChange?: (pageSize: number) => void;
  pageSizeOptions?: number[];
}

function Pagination({
  currentPage,
  totalPages,
  onPageChange,
  totalItems,
  itemsPerPage,
  selectedCount = 0,
  totalOnly = false,
  onPageSizeChange,
  pageSizeOptions = [50, 100, 200],
}: PaginationProps) {
  const { t } = useTranslation();

  if (totalPages <= 1 && !onPageSizeChange) return null;

  // Page range with ellipsis
  let startPage = Math.max(1, currentPage - 2);
  const endPage = Math.min(totalPages, startPage + 4);
  if (endPage - startPage < 4) {
    startPage = Math.max(1, endPage - 4);
  }

  const pages: (number | 'ellipsis-start' | 'ellipsis-end')[] = [];
  if (startPage > 1) {
    pages.push(1);
    if (startPage > 2) pages.push('ellipsis-start');
  }
  for (let i = startPage; i <= endPage; i++) {
    pages.push(i);
  }
  if (endPage < totalPages) {
    if (endPage < totalPages - 1) pages.push('ellipsis-end');
    pages.push(totalPages);
  }

  return (
    <div className="flex items-center justify-between px-1" style={{ height: '30px' }}>
      {/* Left: selected / total */}
      <div className="w-[160px]">
        <span className="text-xs text-gray-500 dark:text-gray-400">
          {totalOnly ? (
            <>
              {t('common.total_label', 'Total')}{' '}
              <span className="font-mono">{totalItems}</span>
            </>
          ) : (
            <>
              {t('common.selected_label', 'Selected')}{' '}
              <span className="font-mono">{selectedCount}</span>
              {' / '}
              {t('common.total_label', 'Total')}{' '}
              <span className="font-mono">{totalItems}</span>
            </>
          )}
        </span>
      </div>

      {/* Center: pagination buttons */}
      <div className="flex items-center gap-1">
        <button
          onClick={() => onPageChange(currentPage - 1)}
          disabled={currentPage === 1}
          className={cn(
            'p-1 rounded-md transition-colors',
            currentPage === 1
              ? 'text-gray-300 dark:text-gray-600 cursor-not-allowed'
              : 'text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-base-200',
          )}
        >
          <ChevronLeft className="w-4 h-4" />
        </button>

        {pages.map((page) =>
          typeof page === 'string' ? (
            <span key={page} className="w-7 h-7 flex items-center justify-center text-xs text-gray-400 dark:text-gray-500">
              ...
            </span>
          ) : (
            <button
              key={page}
              onClick={() => onPageChange(page)}
              className={cn(
                'w-7 h-7 rounded-md text-xs font-medium transition-colors',
                page === currentPage
                  ? 'bg-white dark:bg-base-100 text-gray-900 dark:text-base-content shadow-sm'
                  : 'text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-base-200',
              )}
            >
              {page}
            </button>
          ),
        )}

        <button
          onClick={() => onPageChange(currentPage + 1)}
          disabled={currentPage === totalPages}
          className={cn(
            'p-1 rounded-md transition-colors',
            currentPage === totalPages
              ? 'text-gray-300 dark:text-gray-600 cursor-not-allowed'
              : 'text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-base-200',
          )}
        >
          <ChevronRight className="w-4 h-4" />
        </button>
      </div>

      {/* Right: items per page */}
      {onPageSizeChange ? (
        <div className="flex items-center gap-2 w-[160px] justify-end">
          <span className="text-sm text-gray-600 dark:text-gray-400">
            {t('common.per_page', 'Per page')}
          </span>
          <select
            value={itemsPerPage}
            onChange={(e) => onPageSizeChange(parseInt(e.target.value))}
            className="px-2 py-1 text-sm bg-white dark:bg-base-100 border border-gray-300 dark:border-base-300 rounded-lg text-gray-900 dark:text-base-content focus:outline-none focus:ring-2 focus:ring-blue-500"
          >
            {pageSizeOptions.map((size) => (
              <option key={size} value={size}>{size}</option>
            ))}
          </select>
        </div>
      ) : (
        <div className="w-[160px]" />
      )}
    </div>
  );
}

export default Pagination;
