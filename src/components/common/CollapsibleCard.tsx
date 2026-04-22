/**
 * CollapsibleCard — aligned with AM CollapsibleCard
 * With title icon, enable/disable toggle, expand/collapse animation
 */
import { useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { cn } from '../../utils/cn';

interface CollapsibleCardProps {
  title: string;
  icon: React.ReactNode;
  enabled?: boolean;
  onToggle?: (enabled: boolean) => void;
  children: React.ReactNode;
  defaultExpanded?: boolean;
  rightElement?: React.ReactNode;
  statusText?: string;
}

function CollapsibleCard({
  title,
  icon,
  enabled,
  onToggle,
  children,
  defaultExpanded = false,
  rightElement,
  statusText,
}: CollapsibleCardProps) {
  const [isExpanded, setIsExpanded] = useState(defaultExpanded);

  return (
    <div className="bg-white dark:bg-base-100 rounded-lg shadow-sm border border-gray-100 dark:border-gray-700/50 overflow-hidden transition-all duration-200 hover:shadow-md">
      {/* Header */}
      <div
        className="px-5 py-3 flex items-center justify-between cursor-pointer bg-gray-50/50 dark:bg-gray-800/50 hover:bg-gray-50 dark:hover:bg-gray-700/50 transition-colors"
        onClick={(e) => {
          if ((e.target as HTMLElement).closest('.no-expand')) return;
          setIsExpanded(!isExpanded);
        }}
      >
        <div className="flex items-center gap-3">
          <div className="text-gray-500 dark:text-gray-400">
            {icon}
          </div>
          <span className="font-medium text-sm text-gray-900 dark:text-gray-100">
            {title}
          </span>
          {enabled !== undefined && (
            <div className={cn(
              'text-xs px-2 py-0.5 rounded-full',
              enabled
                ? 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-400'
                : 'bg-gray-100 text-gray-500 dark:bg-gray-600/50 dark:text-gray-300',
            )}>
              {enabled ? 'Enabled' : 'Disabled'}
            </div>
          )}
          {statusText && (
            <span className="text-xs text-gray-400 dark:text-gray-500">{statusText}</span>
          )}
        </div>

        <div className="flex items-center gap-3">
          <div className="no-expand flex items-center gap-3">
            {rightElement}

            {enabled !== undefined && onToggle && (
              <div className="flex items-center" onClick={(e) => e.stopPropagation()}>
                <input
                  type="checkbox"
                  className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500"
                  checked={enabled}
                  onChange={(e) => onToggle(e.target.checked)}
                />
              </div>
            )}
          </div>

          <div className={cn('p-1 rounded-lg hover:bg-gray-200 dark:hover:bg-gray-700 transition-all duration-200', isExpanded ? 'rotate-180' : '')}>
            <ChevronDown size={16} className="text-gray-400" />
          </div>
        </div>
      </div>

      {/* Content */}
      <div
        className={cn(
          'transition-all duration-300 ease-in-out border-t border-gray-100 dark:border-base-200',
          isExpanded ? 'max-h-[2000px] opacity-100' : 'max-h-0 opacity-0 overflow-hidden',
        )}
      >
        <div className="p-5 relative">
          {enabled === false && (
            <div className="absolute inset-0 bg-gray-100/40 dark:bg-black/30 z-10 cursor-not-allowed" />
          )}
          <div className={enabled === false ? 'opacity-60 pointer-events-none select-none' : ''}>
            {children}
          </div>
        </div>
      </div>
    </div>
  );
}

export default CollapsibleCard;
