use serde::{Deserialize, Serialize};

/// Proxy provider credentials + defaults. `kind` discriminates IPRoyal / IPFoxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyProviderConfig {
    /// Proxy provider type: "iproyal" or "ipfoxy". Dispatches URL format in build_proxy_url.
    #[serde(default = "default_kind")]
    pub kind: String,
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
    #[serde(default = "default_max_allocate_retries")]
    pub max_allocate_retries: u32,
}

fn default_kind() -> String {
    "iproyal".to_string()
}
fn default_proxy_host() -> String {
    "geo.iproyal.com".to_string()
}
fn default_proxy_port() -> u16 {
    12321
}
fn default_country() -> String {
    "us".to_string()
}
fn default_max_allocate_retries() -> u32 {
    3
}

impl Default for ProxyProviderConfig {
    fn default() -> Self {
        Self {
            kind: default_kind(),
            host: default_proxy_host(),
            port: default_proxy_port(),
            username: String::new(),
            password: String::new(),
            default_country: default_country(),
            max_allocate_retries: 3,
        }
    }
}

/// Proxy monitor configuration (check intervals + timeouts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyMonitorConfig {
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u64,
    #[serde(default = "default_level0_timeout")]
    pub level0_timeout_secs: u64,
    #[serde(default = "default_level1_timeout")]
    pub level1_timeout_secs: u64,
    #[serde(default = "default_level2_timeout")]
    pub level2_timeout_secs: u64,
}

fn default_check_interval() -> u64 {
    900 // 15 minutes — low cost, frequent health checks are beneficial
}
fn default_level0_timeout() -> u64 {
    5
}
fn default_level1_timeout() -> u64 {
    10
}
fn default_level2_timeout() -> u64 {
    15
}

impl Default for ProxyMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 900,
            level0_timeout_secs: 5,
            level1_timeout_secs: 10,
            level2_timeout_secs: 15,
        }
    }
}
