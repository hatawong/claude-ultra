import React, { useEffect, useRef, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Terminal, X, Trash2, Search, Pause, Play, Bug, Info, AlertTriangle, AlertOctagon } from 'lucide-react';
import { useDebugConsole, type LogEntry, type LogLevel } from '../../stores/useDebugConsole';
import { cn } from '../../utils/cn';

const LEVEL_CONFIG: Record<LogLevel, { color: string; icon: React.ReactNode }> = {
  ERROR: { color: 'text-red-500', icon: <AlertOctagon size={12} /> },
  WARN: { color: 'text-amber-500', icon: <AlertTriangle size={12} /> },
  INFO: { color: 'text-blue-500', icon: <Info size={12} /> },
  DEBUG: { color: 'text-zinc-400', icon: <Bug size={12} /> },
  TRACE: { color: 'text-zinc-600', icon: <Terminal size={12} /> },
};

const LogRow = React.memo(({ log }: { log: LogEntry }) => {
  const [expanded, setExpanded] = useState(false);
  const date = new Date(log.timestamp);
  const timeStr = date.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
    + '.' + date.getMilliseconds().toString().padStart(3, '0');
  const hasFields = Object.keys(log.fields).length > 0;

  return (
    <div className="border-b border-zinc-100 dark:border-white/5 hover:bg-zinc-50 dark:hover:bg-white/5 transition-colors">
      <div
        className={cn("flex gap-2 px-2 py-1 items-start text-[11px]", hasFields && "cursor-pointer")}
        onClick={() => hasFields && setExpanded(!expanded)}
      >
        <span className="text-zinc-400 dark:text-zinc-500 shrink-0 select-none min-w-[85px]">{timeStr}</span>
        <span className={cn("shrink-0 min-w-[50px] font-bold uppercase flex items-center gap-1", LEVEL_CONFIG[log.level]?.color)}>
          {LEVEL_CONFIG[log.level]?.icon}
          {log.level}
        </span>
        <span className="text-zinc-500 dark:text-zinc-400 shrink-0 min-w-[120px] max-w-[120px] truncate font-medium" title={log.target}>
          {log.target.split('::').slice(-2).join('::')}
        </span>
        <span className="flex-1 break-words whitespace-pre-wrap font-medium text-zinc-700 dark:text-zinc-300">
          {log.message}
        </span>
      </div>
      {expanded && hasFields && (
        <div className="px-4 py-2 bg-zinc-50 dark:bg-black/20 text-zinc-600 dark:text-zinc-400 border-t border-zinc-100 dark:border-white/5 text-[11px]">
          <div className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            {Object.entries(log.fields).map(([key, value]) => (
              <React.Fragment key={key}>
                <span className="text-zinc-400 dark:text-zinc-500 text-right">{key}:</span>
                <span className="text-zinc-800 dark:text-zinc-300 break-all select-text font-medium">{value}</span>
              </React.Fragment>
            ))}
          </div>
        </div>
      )}
    </div>
  );
});

interface DebugConsoleProps {
  embedded?: boolean;
}

const DebugConsole: React.FC<DebugConsoleProps> = ({ embedded = false }) => {
  const {
    isOpen, close, logs, clearLogs,
    filter, setFilter,
    searchTerm, setSearchTerm,
    autoScroll, setAutoScroll,
    checkEnabled,
  } = useDebugConsole();

  const scrollRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(320);

  useEffect(() => { checkEnabled(); }, []);

  useEffect(() => {
    if (autoScroll && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [logs, autoScroll, isOpen]);

  const startResizing = (e: React.MouseEvent) => {
    e.preventDefault();
    const handleMouseMove = (e: MouseEvent) => {
      const newHeight = window.innerHeight - e.clientY;
      if (newHeight > 100 && newHeight < window.innerHeight - 100) setHeight(newHeight);
    };
    const stopResizing = () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', stopResizing);
    };
    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', stopResizing);
  };

  const toggleLevel = (level: LogLevel) => {
    if (filter.includes(level)) {
      setFilter(filter.filter(l => l !== level));
    } else {
      setFilter([...filter, level]);
    }
  };

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    const isAtBottom = Math.abs(el.scrollHeight - el.scrollTop - el.clientHeight) < 20;
    if (!isAtBottom && autoScroll) setAutoScroll(false);
    else if (isAtBottom && !autoScroll) setAutoScroll(true);
  };

  const filteredLogs = logs.filter(log => {
    if (!filter.includes(log.level)) return false;
    if (searchTerm && !log.message.toLowerCase().includes(searchTerm.toLowerCase())
      && !log.target.toLowerCase().includes(searchTerm.toLowerCase())) return false;
    return true;
  });

  const content = (
    <div
      className={cn(
        "flex flex-col font-sans transition-colors duration-200",
        "bg-white dark:bg-[#1e1e1e] text-zinc-700 dark:text-zinc-300",
        embedded
          ? "h-full w-full rounded-xl border border-zinc-200 dark:border-white/10 shadow-sm overflow-hidden"
          : "fixed bottom-0 left-0 right-0 border-t border-zinc-200 dark:border-zinc-800 shadow-2xl z-[9999]"
      )}
      style={embedded ? undefined : { height }}
    >
      {!embedded && (
        <div className="h-1 bg-zinc-200 dark:bg-zinc-800 hover:bg-blue-500 cursor-ns-resize transition-colors w-full" onMouseDown={startResizing} />
      )}

      {/* Toolbar */}
      <div className={cn(
        "flex items-center justify-between px-3 py-2 select-none border-b",
        "bg-zinc-50 dark:bg-[#252526] border-zinc-200 dark:border-black/20",
        embedded && "rounded-t-xl"
      )}>
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-2 font-medium text-xs tracking-wide text-zinc-500 dark:text-zinc-400">
            <Terminal size={14} className="opacity-70" /> CONSOLE
          </span>
          <div className="h-4 w-px bg-zinc-200 dark:bg-white/10 mx-1" />
          <div className="flex rounded-md p-0.5 border bg-white dark:bg-black/20 border-zinc-200 dark:border-white/5">
            {(Object.keys(LEVEL_CONFIG) as LogLevel[]).map(level => (
              <button
                key={level}
                onClick={() => toggleLevel(level)}
                className={cn(
                  "px-2.5 py-0.5 text-[10px] uppercase font-bold rounded-[3px] transition-all",
                  filter.includes(level)
                    ? LEVEL_CONFIG[level].color + " bg-zinc-100 dark:bg-white/10 shadow-sm"
                    : "text-zinc-400 dark:text-zinc-600 hover:text-zinc-600"
                )}
              >
                {level}
              </button>
            ))}
          </div>
          <div className="relative group ml-2">
            <Search size={13} className="absolute left-2.5 top-1.5 text-zinc-400" />
            <input
              type="text"
              value={searchTerm}
              onChange={e => setSearchTerm(e.target.value)}
              placeholder="Filter logs..."
              className="border border-transparent rounded-md pl-8 pr-3 py-1 text-xs w-40 focus:w-64 transition-all focus:outline-none
                bg-zinc-100 dark:bg-black/20 text-zinc-800 dark:text-zinc-300
                focus:bg-white dark:focus:bg-black/40 focus:border-zinc-200 dark:focus:border-white/10"
            />
          </div>
        </div>
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => setAutoScroll(!autoScroll)}
            className={cn("p-1.5 rounded-md transition-all",
              autoScroll ? "text-green-600 bg-green-100 dark:bg-green-500/10" : "text-zinc-400 hover:text-zinc-600"
            )}
          >
            {autoScroll ? <Pause size={14} /> : <Play size={14} />}
          </button>
          <button onClick={clearLogs} className="p-1.5 rounded-md text-zinc-400 hover:text-red-500 transition-all">
            <Trash2 size={14} />
          </button>
          {!embedded && (
            <button onClick={close} className="p-1.5 rounded-md text-zinc-400 hover:text-zinc-800 dark:hover:text-white ml-1">
              <X size={14} />
            </button>
          )}
        </div>
      </div>

      {/* Log content */}
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        className={cn(
          "flex-1 overflow-y-auto overflow-x-hidden font-mono text-xs bg-white dark:bg-[#1e1e1e]",
          "[&::-webkit-scrollbar]:w-2 [&::-webkit-scrollbar-thumb]:bg-zinc-300 dark:[&::-webkit-scrollbar-thumb]:bg-[#424242] [&::-webkit-scrollbar-thumb]:rounded-full"
        )}
      >
        {filteredLogs.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full select-none text-zinc-400 dark:text-zinc-600">
            <Terminal size={48} className="mb-4 opacity-20" />
            <p className="text-sm font-medium opacity-50">No logs to display</p>
            <p className="text-xs mt-1 opacity-30">Logs will appear here in real-time</p>
          </div>
        ) : (
          <div className="py-1">
            {filteredLogs.map(log => <LogRow key={log.id} log={log} />)}
          </div>
        )}
      </div>

      {/* Footer */}
      <div className={cn(
        "flex items-center justify-between px-3 py-1.5 border-t text-white text-[10px]",
        "bg-[#007acc] border-[#007acc]",
        embedded && "rounded-b-lg"
      )}>
        <div className="flex items-center gap-4">
          {(Object.keys(LEVEL_CONFIG) as LogLevel[]).map(level => {
            const count = logs.filter(l => l.level === level).length;
            if (count === 0) return null;
            return (
              <span key={level} className="font-medium flex items-center gap-1.5 select-none opacity-90">
                {LEVEL_CONFIG[level].icon} {count}
              </span>
            );
          })}
        </div>
        <span className="opacity-80 flex items-center gap-1">
          <div className="w-1.5 h-1.5 rounded-full bg-white animate-pulse" /> Live
        </span>
      </div>
    </div>
  );

  if (embedded) return content;

  return (
    <AnimatePresence>
      {isOpen && (
        <>
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 bg-black/10 z-[9998]"
            onClick={close}
          />
          <motion.div
            initial={{ y: "100%" }}
            animate={{ y: 0 }}
            exit={{ y: "100%" }}
            transition={{ type: "spring", stiffness: 300, damping: 30 }}
            className="fixed inset-x-0 bottom-0 z-[9999]"
          >
            {content}
          </motion.div>
        </>
      )}
    </AnimatePresence>
  );
};

export default DebugConsole;
