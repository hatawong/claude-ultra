use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const CONFIG_FILE: &str = "config.json";
const OLD_GUI_CONFIG: &str = "gui_config.json";
const OLD_GATEWAY_CONFIG: &str = "gateway_config.json";

// ── AppConfig (top-level) ───────────────────────────────────────────

/// Unified application configuration — stored at ~/.claude-ultra/config.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub email: EmailConfig,
    #[serde(default)]
    pub sms: SmsConfig,
}

fn default_server_url() -> String { "https://api.claude-ultra.com".to_string() }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ui: UiConfig::default(),
            server_url: default_server_url(),
            gateway: GatewayConfig::default(),
            proxy: ProxyConfig::default(),
            email: EmailConfig::default(),
            sms: SmsConfig::default(),
        }
    }
}

// ── UiConfig ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub auto_launch: bool,
    #[serde(default = "default_true")]
    pub auto_check_update: bool,
    #[serde(default = "default_update_interval")]
    pub update_check_interval: i32,
    #[serde(default)]
    pub hidden_menu_items: Vec<String>,
    #[serde(default)]
    pub default_export_path: Option<String>,
}

fn default_language() -> String { "en".to_string() }
fn default_theme() -> String { "dark".to_string() }
fn default_true() -> bool { true }
fn default_update_interval() -> i32 { 24 }

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            language: default_language(),
            theme: default_theme(),
            auto_launch: false,
            auto_check_update: true,
            update_check_interval: 24,
            hidden_menu_items: Vec::new(),
            default_export_path: None,
        }
    }
}

// ── GatewayConfig ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_api_key")]
    pub api_key: String,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,
    #[serde(default = "default_safety_watermark")]
    pub safety_watermark: f64,
    #[serde(default = "default_alert_watermark")]
    pub alert_watermark: f64,
    #[serde(default = "default_true")]
    pub auto_start: bool,
    #[serde(default)]
    pub admin_password: String,
    #[serde(default)]
    pub enable_logging: bool,
    #[serde(default = "default_upstream_base_url")]
    pub upstream_base_url: String,
}

fn default_upstream_base_url() -> String { "https://api.anthropic.com".to_string() }

fn default_port() -> u16 { 9000 }
fn default_bind_address() -> String { "127.0.0.1".to_string() }
fn default_api_key() -> String {
    format!("sk-ultra-{}", uuid::Uuid::new_v4().to_string().replace("-", ""))
}
fn default_max_retries() -> u32 { 3 }
fn default_request_timeout() -> u64 { 300 }
fn default_safety_watermark() -> f64 { 0.6 }
fn default_alert_watermark() -> f64 { 0.8 }

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9000,
            bind_address: default_bind_address(),
            api_key: default_api_key(),
            max_retries: 3,
            request_timeout: 300,
            safety_watermark: 0.6,
            alert_watermark: 0.8,
            auto_start: true,
            admin_password: String::new(),
            enable_logging: false,
            upstream_base_url: default_upstream_base_url(),
        }
    }
}

// ── ProxyConfig ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_proxy_type")]
    pub default_type: String,
    #[serde(default)]
    pub residential: ResidentialProxyConfig,
    #[serde(default)]
    pub mobile: MobileProxyConfig,
}

fn default_proxy_type() -> String { "residential".to_string() }

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            default_type: default_proxy_type(),
            residential: ResidentialProxyConfig::default(),
            mobile: MobileProxyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentialProxyConfig {
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_country")]
    pub default_country: String,
}

fn default_proxy_host() -> String { "geo.iproyal.com".to_string() }
fn default_proxy_port() -> u16 { 12321 }
fn default_country() -> String { "us".to_string() }

impl Default for ResidentialProxyConfig {
    fn default() -> Self {
        Self {
            host: default_proxy_host(),
            port: default_proxy_port(),
            username: String::new(),
            password: String::new(),
            default_country: default_country(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileProxyConfig {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub http_port: u16,
    #[serde(default)]
    pub socks5_port: u16,
    #[serde(default)]
    pub rotate_url: String,
}

impl Default for MobileProxyConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            host: String::new(),
            http_port: 0,
            socks5_port: 0,
            rotate_url: String::new(),
        }
    }
}

// ── EmailConfig ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub mailslurp_api_key: String,
    #[serde(default)]
    pub mailslurp_inbox_id: String,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            domains: Vec::new(),
            mailslurp_api_key: String::new(),
            mailslurp_inbox_id: String::new(),
        }
    }
}

// ── SmsConfig ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsConfig {
    #[serde(default)]
    pub api_key: String,
}

impl Default for SmsConfig {
    fn default() -> Self {
        Self { api_key: String::new() }
    }
}

impl GatewayConfig {
    /// Save gateway config by updating the gateway section of the unified config file.
    pub fn save(&self) {
        if let Ok(mut config) = load_app_config() {
            config.gateway = self.clone();
            let _ = save_app_config(&config);
        }
    }
}

// ── Proxy mode detection ────────────────────────────────────────────

/// Whether the app should use a proxy or connect directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    /// Proxy credentials configured — use ProxyPool for IP rotation.
    Proxied,
    /// No proxy credentials — BoringClient direct connection (user's real IP).
    Direct,
}

/// Check if residential proxy credentials are configured.
pub fn is_proxy_configured(config: &AppConfig) -> bool {
    let r = &config.proxy.residential;
    !r.username.trim().is_empty()
        && !r.password.trim().is_empty()
        && !r.host.trim().is_empty()
        && r.port > 0
}

/// Determine proxy mode from config.
pub fn get_proxy_mode(config: &AppConfig) -> ProxyMode {
    if is_proxy_configured(config) {
        ProxyMode::Proxied
    } else {
        ProxyMode::Direct
    }
}

// ── Load / Save ─────────────────────────────────────────────────────

/// Get config directory (~/.claude-ultra/)
pub fn get_config_dir() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    let dir = home.join(".claude-ultra");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    Ok(dir)
}

/// Load config from ~/.claude-ultra/config.json.
/// On first run, migrates from old gui_config.json + gateway_config.json if present.
pub fn load_app_config() -> Result<AppConfig, String> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join(CONFIG_FILE);

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config: {}", e))?;
        let config: AppConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config: {}", e))?;
        return Ok(config);
    }

    // Migration: merge old files into new unified config
    let mut config = AppConfig::default();
    let mut gui_migrated = false;
    let mut gw_migrated = false;

    // Migrate gui_config.json → ui + server_url
    let old_gui = config_dir.join(OLD_GUI_CONFIG);
    if old_gui.exists() {
        if let Ok(content) = fs::read_to_string(&old_gui) {
            if let Ok(old) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(lang) = old.get("language").and_then(|v| v.as_str()) {
                    config.ui.language = lang.to_string();
                }
                if let Some(theme) = old.get("theme").and_then(|v| v.as_str()) {
                    config.ui.theme = theme.to_string();
                }
                if let Some(v) = old.get("auto_launch").and_then(|v| v.as_bool()) {
                    config.ui.auto_launch = v;
                }
                if let Some(v) = old.get("auto_check_update").and_then(|v| v.as_bool()) {
                    config.ui.auto_check_update = v;
                }
                if let Some(v) = old.get("update_check_interval").and_then(|v| v.as_i64()) {
                    config.ui.update_check_interval = v as i32;
                }
                if let Some(arr) = old.get("hidden_menu_items").and_then(|v| v.as_array()) {
                    config.ui.hidden_menu_items = arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                }
                if let Some(v) = old.get("default_export_path").and_then(|v| v.as_str()) {
                    config.ui.default_export_path = Some(v.to_string());
                }
                if let Some(v) = old.get("server_url").and_then(|v| v.as_str()) {
                    config.server_url = v.to_string();
                }
                gui_migrated = true;
                tracing::info!("Migrated gui_config.json → config.json");
            } else {
                tracing::warn!("Failed to parse gui_config.json, keeping original file");
            }
        }
    }

    // Migrate gateway_config.json → gateway
    let old_gw = config_dir.join(OLD_GATEWAY_CONFIG);
    if old_gw.exists() {
        if let Ok(content) = fs::read_to_string(&old_gw) {
            if let Ok(gw) = serde_json::from_str::<GatewayConfig>(&content) {
                config.gateway = gw;
                gw_migrated = true;
                tracing::info!("Migrated gateway_config.json → config.json");
            } else {
                tracing::warn!("Failed to parse gateway_config.json, keeping original file");
            }
        }
    }

    // Save merged config
    save_app_config_to(&config_path, &config)?;

    // Only remove old files that were successfully migrated
    if gui_migrated {
        let _ = fs::remove_file(&old_gui);
    }
    if gw_migrated {
        let _ = fs::remove_file(&old_gw);
    }

    Ok(config)
}

/// Save config to file
pub fn save_app_config(config: &AppConfig) -> Result<(), String> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join(CONFIG_FILE);
    save_app_config_to(&config_path, config)
}

fn save_app_config_to(path: &Path, config: &AppConfig) -> Result<(), String> {
    // Atomic write + 0600 permissions (config contains credentials like IPRoyal)
    crate::modules::secure_fs::secure_write_json(path, config)
}

// ── Tauri Commands ──────────────────────────────────────────────────

#[tauri::command]
pub async fn load_config() -> Result<AppConfig, String> {
    load_app_config()
}

#[tauri::command]
pub async fn save_config(config: AppConfig) -> Result<(), String> {
    save_app_config(&config)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude_ultra_config_test_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.ui.language, "en");
        assert_eq!(config.ui.theme, "dark");
        assert!(!config.ui.auto_launch);
        assert!(config.ui.auto_check_update);
        assert_eq!(config.gateway.port, 9000);
        assert!(config.gateway.api_key.starts_with("sk-ultra-"));
        assert_eq!(config.proxy.residential.host, "geo.iproyal.com");
        assert_eq!(config.proxy.residential.port, 12321);
        assert!(config.email.domains.is_empty());
        assert!(config.sms.api_key.is_empty());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let config = AppConfig {
            ui: UiConfig {
                language: "zh".to_string(),
                theme: "light".to_string(),
                auto_launch: true,
                auto_check_update: false,
                update_check_interval: 12,
                hidden_menu_items: vec!["/monitor".to_string()],
                default_export_path: Some("/tmp/export".to_string()),
            },
            server_url: "https://api.example.com".to_string(),
            gateway: GatewayConfig {
                port: 8080,
                ..Default::default()
            },
            proxy: ProxyConfig {
                residential: ResidentialProxyConfig {
                    username: "testuser".to_string(),
                    password: "testpass".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            email: EmailConfig {
                domains: vec!["test.com".to_string()],
                mailslurp_api_key: "sk_test".to_string(),
                mailslurp_inbox_id: "inbox1".to_string(),
            },
            sms: SmsConfig { api_key: "sms_key".to_string() },
        };

        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.ui.language, "zh");
        assert_eq!(restored.server_url, "https://api.example.com");
        assert_eq!(restored.gateway.port, 8080);
        assert_eq!(restored.proxy.residential.username, "testuser");
        assert_eq!(restored.email.domains, vec!["test.com"]);
        assert_eq!(restored.sms.api_key, "sms_key");
    }

    #[test]
    fn test_deserialize_with_missing_fields() {
        let json = r#"{"ui":{"language":"ja"}}"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.ui.language, "ja");
        assert_eq!(config.ui.theme, "dark"); // default
        assert_eq!(config.gateway.port, 9000); // default
        assert_eq!(config.proxy.residential.host, "geo.iproyal.com"); // default
    }

    #[test]
    fn test_deserialize_empty_object() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.ui.language, "en");
        assert_eq!(config.gateway.port, 9000);
        assert_eq!(config.server_url, "https://api.claude-ultra.com");
    }

    #[test]
    fn test_save_and_load() {
        let dir = TestDir::new();
        let path = dir.path.join(CONFIG_FILE);

        let config = AppConfig {
            ui: UiConfig {
                language: "ko".to_string(),
                hidden_menu_items: vec!["/security".to_string()],
                ..Default::default()
            },
            proxy: ProxyConfig {
                residential: ResidentialProxyConfig {
                    username: "myuser".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        save_app_config_to(&path, &config).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let loaded: AppConfig = serde_json::from_str(&content).unwrap();

        assert_eq!(loaded.ui.language, "ko");
        assert_eq!(loaded.ui.hidden_menu_items, vec!["/security"]);
        assert_eq!(loaded.proxy.residential.username, "myuser");
    }

    #[test]
    fn test_gateway_config_defaults() {
        let config = GatewayConfig::default();
        assert!(config.enabled);
        assert_eq!(config.port, 9000);
        assert_eq!(config.bind_address, "127.0.0.1");
        assert!(config.api_key.starts_with("sk-ultra-"));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.request_timeout, 300);
        assert!((config.safety_watermark - 0.6).abs() < f64::EPSILON);
        assert!((config.alert_watermark - 0.8).abs() < f64::EPSILON);
    }
}
