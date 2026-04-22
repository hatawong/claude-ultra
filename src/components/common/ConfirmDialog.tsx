import { useTranslation } from 'react-i18next';

interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmText?: string;
  confirmColor?: 'red' | 'blue' | 'green';
  onConfirm: () => void;
  onCancel: () => void;
}

function ConfirmDialog({ title, message, confirmText, confirmColor = 'red', onConfirm, onCancel }: ConfirmDialogProps) {
  const { t } = useTranslation();
  const colorClasses = {
    red: 'bg-red-500 hover:bg-red-600',
    blue: 'bg-blue-500 hover:bg-blue-600',
    green: 'bg-green-500 hover:bg-green-600',
  };

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50" onClick={onCancel}>
      <div className="bg-white dark:bg-base-100 rounded-xl shadow-2xl w-[400px] p-6 border border-gray-200 dark:border-base-300" onClick={(e) => e.stopPropagation()}>
        <h3 className="text-sm font-semibold text-gray-900 dark:text-base-content mb-2">{title}</h3>
        <p className="text-xs text-gray-500 dark:text-gray-400 mb-5">{message}</p>
        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="px-4 py-1.5 text-xs font-medium bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 rounded-lg hover:bg-gray-200 dark:hover:bg-base-300 transition-colors"
          >
            {t('common.cancel', 'Cancel')}
          </button>
          <button
            onClick={onConfirm}
            className={`px-4 py-1.5 text-xs font-medium text-white rounded-lg transition-colors ${colorClasses[confirmColor]}`}
          >
            {confirmText || t('common.confirm', 'Confirm')}
          </button>
        </div>
      </div>
    </div>
  );
}

export default ConfirmDialog;
