//! Log Bridge — captures tracing logs and emits them to the frontend via Tauri events.
//! Uses a global ring buffer with parking_lot::RwLock.

use parking_lot::RwLock;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::Emitter;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const MAX_BUFFER_SIZE: usize = 5000;

static LOG_BRIDGE_ENABLED: AtomicBool = AtomicBool::new(false);
static LOG_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
static LOG_BUFFER: OnceLock<Arc<RwLock<VecDeque<LogEntry>>>> = OnceLock::new();
static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

fn get_log_buffer() -> &'static Arc<RwLock<VecDeque<LogEntry>>> {
    LOG_BUFFER.get_or_init(|| Arc::new(RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE))))
}

/// Log entry sent to frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: u64,
    pub timestamp: i64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: std::collections::HashMap<String, String>,
}

/// Initialize the log bridge with app handle (call from setup).
pub fn init_log_bridge(app_handle: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app_handle);
}

/// Get reference to the global app handle (for other modules to emit events).
pub fn get_app_handle() -> Option<&'static tauri::AppHandle> {
    APP_HANDLE.get()
}

/// Enable log bridging and emit buffered logs to frontend.
pub fn enable_log_bridge() {
    LOG_BRIDGE_ENABLED.store(true, Ordering::SeqCst);
    // Flush buffered logs to frontend
    if let Some(handle) = APP_HANDLE.get() {
        let buffer = get_log_buffer().read();
        for entry in buffer.iter() {
            let _ = handle.emit("log-event", entry.clone());
        }
    }
}

/// Disable log bridging.
pub fn disable_log_bridge() {
    LOG_BRIDGE_ENABLED.store(false, Ordering::SeqCst);
}

/// Check if log bridging is enabled.
pub fn is_log_bridge_enabled() -> bool {
    LOG_BRIDGE_ENABLED.load(Ordering::SeqCst)
}

/// Get all buffered logs.
pub fn get_buffered_logs() -> Vec<LogEntry> {
    get_log_buffer().read().iter().cloned().collect()
}

/// Clear log buffer.
pub fn clear_log_buffer() {
    get_log_buffer().write().clear();
}

/// Add a log entry to the buffer and emit to frontend.
pub fn add_log_entry(entry: LogEntry) {
    {
        let mut buffer = get_log_buffer().write();
        while buffer.len() >= MAX_BUFFER_SIZE {
            buffer.pop_front();
        }
        buffer.push_back(entry.clone());
    }
    // Emit to frontend if bridge is enabled and app handle is available
    if LOG_BRIDGE_ENABLED.load(Ordering::Relaxed) {
        if let Some(handle) = APP_HANDLE.get() {
            let _ = handle.emit("log-event", entry);
        }
    }
}

// ── Field visitor ─────────────────────────────────────────

struct FieldVisitor {
    message: Option<String>,
    fields: std::collections::HashMap<String, String>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self {
            message: None,
            fields: std::collections::HashMap::new(),
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let value_str = format!("{:?}", value);
        if field.name() == "message" {
            self.message = Some(value_str.trim_matches('"').to_string());
        } else {
            self.fields.insert(field.name().to_string(), value_str);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

// ── Tracing Layer ─────────────────────────────────────────

/// Tracing Layer that bridges logs to the ring buffer.
pub struct LogBridgeLayer;

impl LogBridgeLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogBridgeLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for LogBridgeLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if !LOG_BRIDGE_ENABLED.load(Ordering::Relaxed) {
            return;
        }

        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN",
            Level::INFO => "INFO",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };

        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);

        let message = visitor.message.unwrap_or_default();
        if message.is_empty() && visitor.fields.is_empty() {
            return;
        }

        let entry = LogEntry {
            id: LOG_ID_COUNTER.fetch_add(1, Ordering::SeqCst),
            timestamp: chrono::Utc::now().timestamp_millis(),
            level: level.to_string(),
            target: metadata.target().to_string(),
            message,
            fields: visitor.fields,
        };

        add_log_entry(entry);
    }
}

// ── Tauri Commands ────────────────────────────────────────

#[tauri::command]
pub fn enable_debug_console() {
    enable_log_bridge();
}

#[tauri::command]
pub fn disable_debug_console() {
    disable_log_bridge();
}

#[tauri::command]
pub fn is_debug_console_enabled() -> bool {
    is_log_bridge_enabled()
}

#[tauri::command]
pub fn get_debug_console_logs() -> Vec<LogEntry> {
    get_buffered_logs()
}

#[tauri::command]
pub fn clear_debug_console_logs() {
    clear_log_buffer();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize log_bridge tests — they share a global LOG_BUFFER
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_buffer() {
        clear_log_buffer();
        LOG_BRIDGE_ENABLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_enable_disable() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_buffer();
        assert!(!is_log_bridge_enabled());
        enable_log_bridge();
        assert!(is_log_bridge_enabled());
        disable_log_bridge();
        assert!(!is_log_bridge_enabled());
    }

    #[test]
    fn test_add_and_get_logs() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_buffer();
        let entry = LogEntry {
            id: 0,
            timestamp: 1000,
            level: "INFO".to_string(),
            target: "test::module".to_string(),
            message: "hello world".to_string(),
            fields: std::collections::HashMap::new(),
        };
        add_log_entry(entry);
        let logs = get_buffered_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "hello world");
    }

    #[test]
    fn test_ring_buffer_evicts_oldest() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_buffer();
        // Add exactly MAX_BUFFER_SIZE + 100 entries
        for i in 0..(MAX_BUFFER_SIZE + 100) {
            add_log_entry(LogEntry {
                id: 10000 + i as u64,
                timestamp: i as i64,
                level: "INFO".to_string(),
                target: "test".to_string(),
                message: format!("evict-{}", i),
                fields: std::collections::HashMap::new(),
            });
        }
        let logs = get_buffered_logs();
        assert_eq!(logs.len(), MAX_BUFFER_SIZE);
        // First entry should be the 100th (0-99 evicted)
        assert_eq!(logs[0].message, "evict-100");
        assert_eq!(logs[logs.len() - 1].message, format!("evict-{}", MAX_BUFFER_SIZE + 99));
    }

    #[test]
    fn test_clear_buffer() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_buffer();
        add_log_entry(LogEntry {
            id: 0,
            timestamp: 1000,
            level: "INFO".to_string(),
            target: "test".to_string(),
            message: "test".to_string(),
            fields: std::collections::HashMap::new(),
        });
        assert!(!get_buffered_logs().is_empty());
        clear_log_buffer();
        assert!(get_buffered_logs().is_empty());
    }

    #[test]
    fn test_log_entry_serialization() {
        let entry = LogEntry {
            id: 42,
            timestamp: 1712188800000,
            level: "ERROR".to_string(),
            target: "gateway::handler".to_string(),
            message: "request failed".to_string(),
            fields: {
                let mut m = std::collections::HashMap::new();
                m.insert("status".to_string(), "503".to_string());
                m
            },
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["id"], 42);
        assert_eq!(json["level"], "ERROR");
        assert_eq!(json["target"], "gateway::handler");
        assert_eq!(json["message"], "request failed");
        assert_eq!(json["fields"]["status"], "503");
    }
}
