import { useState, useEffect, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { CheckCircle, XCircle, Info, AlertTriangle, X } from 'lucide-react';

export type ToastType = 'success' | 'error' | 'info' | 'warning';

export interface ToastItem {
  id: string;
  message: string;
  type: ToastType;
  duration?: number;
}

let toastCounter = 0;
let addToastExternal: ((message: string, type: ToastType, duration?: number) => void) | null = null;

export const showToast = (message: string, type: ToastType = 'info', duration: number = 3000) => {
  if (addToastExternal) {
    addToastExternal(message, type, duration);
  }
};

// ─── Single Toast ─────────────────────────────────────────

function ToastItem({ id, message, type, duration = 3000, onClose }: {
  id: string; message: string; type: ToastType; duration?: number; onClose: (id: string) => void;
}) {
  const [isVisible, setIsVisible] = useState(false);
  const [isHovered, setIsHovered] = useState(false);

  useEffect(() => {
    requestAnimationFrame(() => setIsVisible(true));
  }, []);

  useEffect(() => {
    if (duration <= 0 || isHovered) return;
    const timer = setTimeout(() => {
      setIsVisible(false);
      setTimeout(() => onClose(id), 300);
    }, duration);
    return () => clearTimeout(timer);
  }, [duration, id, onClose, isHovered]);

  const icon = {
    success: <CheckCircle className="w-5 h-5 text-green-500" />,
    error: <XCircle className="w-5 h-5 text-red-500" />,
    warning: <AlertTriangle className="w-5 h-5 text-yellow-500" />,
    info: <Info className="w-5 h-5 text-blue-500" />,
  }[type];

  const border = {
    success: 'border-green-100 dark:border-green-900/30',
    error: 'border-red-100 dark:border-red-900/30',
    warning: 'border-yellow-100 dark:border-yellow-900/30',
    info: 'border-blue-100 dark:border-blue-900/30',
  }[type];

  return (
    <div
      className={`flex items-center gap-3 px-4 py-3 rounded-xl shadow-lg border bg-white dark:bg-base-100 transition-all duration-300 transform ${border} ${isVisible ? 'opacity-100 translate-y-0' : 'opacity-0 translate-y-2'}`}
      style={{ minWidth: '300px' }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
    >
      {icon}
      <p className="flex-1 text-sm font-medium text-gray-700 dark:text-base-content">{message}</p>
      <button
        onClick={() => { setIsVisible(false); setTimeout(() => onClose(id), 300); }}
        className="text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
      >
        <X className="w-4 h-4" />
      </button>
    </div>
  );
}

// ─── Toast Container (mount once in App) ──────────────────

export function ToastContainer() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);

  const addToast = useCallback((message: string, type: ToastType, duration?: number) => {
    const id = `toast-${Date.now()}-${toastCounter++}`;
    setToasts(prev => [...prev, { id, message, type, duration }]);
  }, []);

  const removeToast = useCallback((id: string) => {
    setToasts(prev => prev.filter(t => t.id !== id));
  }, []);

  useEffect(() => {
    addToastExternal = addToast;
    return () => { addToastExternal = null; };
  }, [addToast]);

  return createPortal(
    <div className="fixed top-24 right-8 z-[200] flex flex-col gap-3 pointer-events-none">
      <div className="flex flex-col gap-3 pointer-events-auto">
        {toasts.map(toast => (
          <ToastItem key={toast.id} {...toast} onClose={removeToast} />
        ))}
      </div>
    </div>,
    document.body
  );
}

// ─── Legacy default export (backward compat) ─────────────

export default function LegacyToast({ message, onClose, duration = 2000 }: {
  message: string; onClose: () => void; duration?: number;
}) {
  useEffect(() => {
    const timer = setTimeout(onClose, duration);
    return () => clearTimeout(timer);
  }, [onClose, duration]);

  return (
    <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-[70] px-4 py-2 bg-gray-800 dark:bg-gray-200 text-white dark:text-gray-800 text-xs font-medium rounded-lg shadow-lg">
      {message}
    </div>
  );
}
