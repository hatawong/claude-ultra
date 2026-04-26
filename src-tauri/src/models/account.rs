use serde::{Deserialize, Serialize};
use std::path::Path;

// ─── Sub-structures ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDevice {
    pub device_id: String,
    pub android_id: String,
    pub platform: String,
    pub manufacturer: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    pub os_version: String,
    pub release_version: String,
    pub locale: String,
    pub timezone: String,
    pub build_tags: String,
    pub carrier_name: String,
    pub carrier_country: String,
    pub app_version: String,
    pub anthropic_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_addresses: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidClient {
    pub device: AndroidDevice,
    pub session_key: String,
    pub routing_hint: String,
    pub last_activity: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieData {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: f64,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebClient {
    #[serde(default)]
    pub cookies: Vec<CookieData>,
    #[serde(default)]
    pub local_storage: std::collections::HashMap<String, String>,
    pub last_activity: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliClient {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default, deserialize_with = "deserialize_scopes")]
    pub scopes: Vec<String>,
    pub last_activity: Option<i64>,
    #[serde(default)]
    pub device_id: String,
}

/// Generate a new device_id (64-char hex, matches CC CLI format).
pub fn generate_device_id() -> String {
    use sha2::{Sha256, Digest};
    let salt = uuid::Uuid::new_v4().to_string();
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Return existing device_id if non-empty, otherwise generate a new one.
pub fn ensure_device_id(existing: &str) -> String {
    if existing.is_empty() {
        generate_device_id()
    } else {
        existing.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySection {
    pub session_id: String,
    pub host: String,
    pub last_ip: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub isp: Option<String>,
    pub quality: Option<String>,
    pub last_checked: Option<i64>,
    /// residential | mobile
    #[serde(default, rename = "type")]
    pub proxy_type: Option<String>,
    /// 24h, 1h, etc.
    #[serde(default)]
    pub lifetime: Option<String>,
    /// Session creation timestamp (ms)
    #[serde(default)]
    pub created_at: Option<i64>,
}

impl ProxySection {
    /// Build a proxy URL for upstream connections.
    pub fn to_url(&self) -> String {
        format!("http://{}", self.host)
    }
}

/// Static residential proxy credentials. Bound 1:1 to the account; used when
/// route_mode == "static". Independent from ProxySection (which holds runtime
/// IP verification state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticProxy {
    /// Protocol: "socks5" or "http". No socks5h in this round.
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    // No country field: account identity country is Account.country, runtime
    // measured country is ProxySection.country.
}

// ─── Account V3 ──────────────────────────────────────────────

fn default_free() -> String {
    "free".to_string()
}

fn default_manual() -> String {
    "manual".to_string()
}

fn default_route_mode() -> String {
    "proxy".to_string()
}

/// Account V3 — unified data structure for all account lifecycle stages.
/// Compatible with V1 JSON (missing fields use serde defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    // ── Core identity ────────────────────────
    pub account_id: String,
    pub email: String,
    #[serde(default)]
    pub phone_number: String,
    #[serde(default)]
    pub full_name: String,
    pub custom_label: Option<String>,
    #[serde(default)]
    pub account_uuid: String,
    #[serde(default)]
    pub org_id: String,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub created_at: i64,

    // ── Routing (per-account) ───────────────
    /// Routing mode: "proxy" (default) | "static" | "vercel" | "direct".
    /// "static" dispatches via per-account `static_proxy` through
    /// `state.client.proxy(url)` independently from the dynamic ProxyPool.
    /// When `static_proxy` is missing the gateway falls through the
    /// Vercel/Proxy/Direct chain (see `gateway/route.rs::resolve_route`);
    /// paid/configured transports take priority over Direct (real-IP exposure).
    #[serde(default = "default_route_mode")]
    pub route_mode: String,
    /// Target country for proxy mode. None = fallback to self.country.
    #[serde(default)]
    pub route_country: Option<String>,

    // ── Plan ─────────────────────────────────
    #[serde(default = "default_free")]
    pub subscription_type: String,
    pub rate_limit_tier: Option<String>,
    pub subscription_renew_at: Option<i64>,
    pub subscription_created_at: Option<i64>,
    pub billing_type: Option<String>,
    #[serde(default)]
    pub has_extra_usage_enabled: bool,

    // ── Login method ────────────────────────
    #[serde(default = "default_manual")]
    pub login_method: String,

    // ── Status (AM dual-layer) ───────────────
    #[serde(default)]
    pub disabled: bool,
    pub disabled_reason: Option<String>,
    pub disabled_at: Option<i64>,
    #[serde(default)]
    pub user_disabled: bool,
    pub user_disabled_reason: Option<String>,
    pub user_disabled_at: Option<i64>,

    // ── Proxy ────────────────────────────────
    pub proxy: Option<ProxySection>,

    /// Static proxy credentials. Used when route_mode == "static".
    /// Coexists with `proxy: ProxySection` (runtime state) — they are not
    /// alternatives but complementary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_proxy: Option<StaticProxy>,

    // ── Utilization ───
    pub utilization: Option<serde_json::Value>,

    // ── Three clients ────────────────────────
    pub android: Option<AndroidClient>,
    pub web: Option<WebClient>,
    pub cli: Option<CliClient>,
}

impl Account {
    /// Check if this account is available for proxy use.
    #[allow(dead_code)]
    pub fn is_proxy_available(&self) -> bool {
        !self.disabled && !self.user_disabled && self.cli.is_some()
    }

    /// Resolve effective target country for proxy routing.
    /// Precedence: `route_country` (explicit override) → `country` → "us".
    /// Returns ISO 3166-1 alpha-2 lowercase.
    pub fn resolve_proxy_country(&self) -> String {
        self.route_country.as_deref()
            .or(self.country.as_deref())
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "us".to_string())
    }

    pub fn resolve_country(&self) -> String {
        if let Some(ref c) = self.country {
            let trimmed = c.trim();
            if !trimmed.is_empty() {
                return trimmed.to_lowercase();
            }
        }
        "us".to_string()
    }
}

// ─── Directory loading ───────────────────────────────────────

/// Load and parse all account JSON files from a directory.
/// Skips files that fail to parse.
pub fn list_accounts_in_dir(accounts_dir: &Path) -> Result<Vec<Account>, String> {
    if !accounts_dir.exists() {
        return Ok(vec![]);
    }

    let entries = std::fs::read_dir(accounts_dir)
        .map_err(|e| format!("Failed to read accounts directory: {}", e))?;

    let mut accounts = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Warning: failed to read {}: {}", path.display(), e);
                    continue;
                }
            };
            match serde_json::from_str::<Account>(&content) {
                Ok(account) => accounts.push(account),
                Err(e) => {
                    eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                    continue;
                }
            }
        }
    }

    accounts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(accounts)
}

fn get_accounts_dir() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude-ultra").join("accounts"))
}

// NOTE: Legacy `region` field migration removed (no live users carry it;
// existing JSON was cleaned manually via `jq 'del(.region)'`).

#[tauri::command]
pub async fn list_accounts() -> Result<Vec<Account>, String> {
    let accounts_dir = get_accounts_dir()?;
    let mut accounts = list_accounts_in_dir(&accounts_dir)?;

    // Filter out shell accounts (being created, all three clients are empty)
    accounts.retain(|a| a.android.is_some() || a.web.is_some() || a.cli.is_some());

    // Apply custom order if accounts_order.json exists
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    let order_path = home.join(".claude-ultra").join("accounts_order.json");
    if let Ok(content) = std::fs::read_to_string(&order_path) {
        if let Ok(order) = serde_json::from_str::<Vec<String>>(&content) {
            let pos_map: std::collections::HashMap<&str, usize> = order
                .iter()
                .enumerate()
                .map(|(i, id)| (id.as_str(), i))
                .collect();
            accounts.sort_by(|a, b| {
                let pa = pos_map.get(a.account_id.as_str()).copied().unwrap_or(usize::MAX);
                let pb = pos_map.get(b.account_id.as_str()).copied().unwrap_or(usize::MAX);
                pa.cmp(&pb).then(a.created_at.cmp(&b.created_at))
            });
        }
    }

    Ok(accounts)
}

#[tauri::command]
pub async fn delete_account(
    id: String,
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<(), String> {
    use crate::modules::account_change::AccountChange;
    state.account_manager.delete(&id).await?;
    state.account_manager.route_change(&id, AccountChange::Deleted).await;
    Ok(())
}

#[tauri::command]
pub async fn update_account_label(
    id: String,
    label: String,
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<(), String> {
    let label_value = if label.is_empty() { None } else { Some(label) };
    state.account_manager.set_label(&id, label_value).await
}

#[tauri::command]
pub async fn toggle_user_status(
    id: String,
    enable: bool,
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<(), String> {
    let reason = if enable { None } else { Some("Disabled manually by user".to_string()) };
    state.account_manager.set_user_disabled(&id, reason).await
}

#[tauri::command]
pub async fn import_accounts(
    json: String,
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<ImportResult, String> {
    let accounts: Vec<Account> = serde_json::from_str(&json)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let mut success = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    use crate::modules::account_change::AccountChange;
    for account in accounts {
        if account.account_id.is_empty() || account.email.is_empty() {
            failed += 1;
            continue;
        }
        let aid = account.account_id.clone();
        match state.account_manager.write_new(&account).await {
            Ok(()) => {
                success += 1;
                state.account_manager.route_change(&aid, AccountChange::Created).await;
            }
            Err(e) if e.contains("already exists") => skipped += 1,
            Err(_) => failed += 1,
        }
    }

    Ok(ImportResult { success, skipped, failed })
}

#[tauri::command]
pub async fn reorder_accounts(
    ids: Vec<String>,
) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    let config_path = home.join(".claude-ultra").join("accounts_order.json");
    let content = serde_json::to_string_pretty(&ids)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&config_path, content)
        .map_err(|e| format!("Failed to write order: {}", e))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub success: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Deserialize scopes: accepts both string ("a b c") and array (["a", "b", "c"])
fn deserialize_scopes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct ScopesVisitor;
    impl<'de> de::Visitor<'de> for ScopesVisitor {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or array of strings")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.split_whitespace().map(|s| s.to_string()).collect())
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut v = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                v.push(s);
            }
            Ok(v)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }
    deserializer.deserialize_any(ScopesVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use once_cell::sync::Lazy;

    static TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn mk_account() -> Account {
        Account {
            account_id: "test".to_string(),
            email: String::new(),
            phone_number: String::new(),
            full_name: String::new(),
            custom_label: None,
            account_uuid: String::new(),
            org_id: String::new(),
            country: None,
            created_at: 0,
            subscription_type: "free".to_string(),
            rate_limit_tier: None,
            subscription_renew_at: None,
            subscription_created_at: None,
            billing_type: None,
            has_extra_usage_enabled: false,
            login_method: "manual".to_string(),
            disabled: false,
            disabled_reason: None,
            disabled_at: None,
            user_disabled: false,
            user_disabled_reason: None,
            user_disabled_at: None,
            proxy: None,
            static_proxy: None,
            utilization: None,
            route_mode: "proxy".to_string(),
            route_country: None,
            android: None,
            web: None,
            cli: None,
        }
    }

    #[test]
    fn test_resolve_country_non_empty() {
        let mut a = mk_account();
        a.country = Some("jp".to_string());
        assert_eq!(a.resolve_country(), "jp");
    }

    #[test]
    fn test_resolve_country_default_to_us() {
        let a = mk_account();
        assert_eq!(a.resolve_country(), "us");
    }

    #[test]
    fn test_resolve_country_empty_falls_back_us() {
        let mut a = mk_account();
        a.country = Some("   ".to_string());
        assert_eq!(a.resolve_country(), "us");
    }

    struct TestDataDir {
        path: PathBuf,
    }

    impl TestDataDir {
        fn new() -> Self {
            let temp_path = std::env::temp_dir().join(format!(
                "claude_ultra_test_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            ));
            std::fs::create_dir_all(&temp_path).expect("Failed to create temp dir");
            Self { path: temp_path }
        }
        fn path(&self) -> &PathBuf {
            &self.path
        }
    }

    impl Drop for TestDataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(dir: &Path, filename: &str, content: &str) {
        std::fs::write(dir.join(filename), content).unwrap();
    }

    // ── V3 full JSON ──────────────────────────────────────────

    const V3_JSON: &str = r#"{
        "accountId": "ac_v3test",
        "email": "v3@test.com",
        "fullName": "V3 User",
        "phoneNumber": "+1555000",
        "customLabel": "test label",
        "accountUuid": "uuid-v3",
        "orgId": "org-v3",
        "region": "US",
        "createdAt": 2000000000000,
        "subscriptionType": "max",
        "rateLimitTier": "default_claude_max_20x",
        "subscriptionRenewAt": 2000100000000,
        "subscriptionCreatedAt": 2000000000000,
        "billingType": "stripe_subscription",
        "hasExtraUsageEnabled": true,
        "disabled": false,
        "disabledReason": null,
        "disabledAt": null,
        "userDisabled": false,
        "userDisabledReason": null,
        "userDisabledAt": null,
        "proxy": null,
        "quota": null,
        "android": {
            "device": {
                "deviceId": "dv_test", "androidId": "aid", "platform": "android",
                "manufacturer": "Google", "model": "Pixel 8", "osVersion": "34",
                "releaseVersion": "14", "locale": "en-US", "timezone": "UTC",
                "buildTags": "release-keys", "carrierName": "TMo", "carrierCountry": "us",
                "appVersion": "1.3", "anthropicDeviceId": "uuid-dev"
            },
            "sessionKey": "sk-v3-session",
            "routingHint": "rh-v3",
            "lastActivity": null
        },
        "web": null,
        "cli": {
            "accessToken": "sk-ant-oat01-v3test",
            "refreshToken": "sk-ant-ort01-v3test",
            "expiresAt": 2000003600000,
            "scopes": ["user:inference", "user:profile"],
            "lastActivity": null
        }
    }"#;

    #[test]
    fn test_v3_full_json_deserializes() {
        let account: Account = serde_json::from_str(V3_JSON).unwrap();
        assert_eq!(account.account_id, "ac_v3test");
        assert_eq!(account.subscription_type, "max");
        assert_eq!(account.rate_limit_tier, Some("default_claude_max_20x".to_string()));
        assert!(account.has_extra_usage_enabled);
        assert_eq!(account.custom_label, Some("test label".to_string()));
    }

    #[test]
    fn test_v3_android_sub_object() {
        let account: Account = serde_json::from_str(V3_JSON).unwrap();
        let android = account.android.as_ref().unwrap();
        assert_eq!(android.session_key, "sk-v3-session");
        assert_eq!(android.routing_hint, "rh-v3");
        assert_eq!(android.device.model, "Pixel 8");
    }

    #[test]
    fn test_v3_cli_sub_object() {
        let account: Account = serde_json::from_str(V3_JSON).unwrap();
        let cli = account.cli.as_ref().unwrap();
        assert_eq!(cli.access_token, "sk-ant-oat01-v3test");
        assert_eq!(cli.refresh_token, "sk-ant-ort01-v3test");
        assert_eq!(cli.expires_at, 2000003600000);
        assert_eq!(cli.scopes, vec!["user:inference", "user:profile"]);
    }

    #[test]
    fn test_v3_disabled_account() {
        let json = r#"{
            "accountId": "ac_dis", "email": "dis@t.com",
            "disabled": true, "disabledReason": "OAuth token invalid",
            "disabledAt": 1700000000000
        }"#;
        let account: Account = serde_json::from_str(json).unwrap();
        assert!(account.disabled);
        assert_eq!(account.disabled_reason, Some("OAuth token invalid".to_string()));
        assert_eq!(account.disabled_at, Some(1700000000000));
    }

    #[test]
    fn test_v3_user_disabled_account() {
        let json = r#"{
            "accountId": "ac_pd", "email": "pd@t.com",
            "userDisabled": true, "userDisabledReason": "user manual stop"
        }"#;
        let account: Account = serde_json::from_str(json).unwrap();
        assert!(account.user_disabled);
        assert!(!account.disabled);
    }

    // ── is_proxy_available ────────────────────────────────────

    #[test]
    fn test_is_proxy_available_with_cli() {
        let account: Account = serde_json::from_str(V3_JSON).unwrap();
        assert!(account.is_proxy_available());
    }

    #[test]
    fn test_is_proxy_available_no_cli() {
        let json = r#"{"accountId":"a","email":"e"}"#;
        let account: Account = serde_json::from_str(json).unwrap();
        assert!(!account.is_proxy_available());
    }

    #[test]
    fn test_is_proxy_available_disabled() {
        let json = r#"{"accountId":"a","email":"e","disabled":true,"cli":{"accessToken":"t","refreshToken":"r","expiresAt":0}}"#;
        let account: Account = serde_json::from_str(json).unwrap();
        assert!(!account.is_proxy_available());
    }

    #[test]
    fn test_is_proxy_available_user_disabled() {
        let json = r#"{"accountId":"a","email":"e","userDisabled":true,"cli":{"accessToken":"t","refreshToken":"r","expiresAt":0}}"#;
        let account: Account = serde_json::from_str(json).unwrap();
        assert!(!account.is_proxy_available());
    }

    // ── 6. Extra fields ignored (forward compat) ─────────────

    #[test]
    fn test_extra_unknown_fields_ignored() {
        let json = r#"{
            "accountId": "ac_future", "email": "f@t.com",
            "futureField": "should be ignored",
            "billing": {"stripe_id": "cus_xxx"}
        }"#;
        let result = serde_json::from_str::<Account>(json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_missing_required_field_fails() {
        let json = r#"{"email": "no_id@t.com"}"#;
        let result = serde_json::from_str::<Account>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let account: Account = serde_json::from_str(V3_JSON).unwrap();
        let serialized = serde_json::to_string(&account).unwrap();
        let restored: Account = serde_json::from_str(&serialized).unwrap();
        assert_eq!(account.account_id, restored.account_id);
        assert_eq!(account.subscription_type, restored.subscription_type);
    }

    // ── StaticProxy ──────────────────────────────────────────

    #[test]
    fn test_static_proxy_default_none() {
        // Legacy Account JSON without staticProxy field deserializes with None (backward compat).
        let json = r#"{"accountId":"a","email":"e@t.com"}"#;
        let account: Account = serde_json::from_str(json).unwrap();
        assert!(account.static_proxy.is_none());
    }

    #[test]
    fn test_static_proxy_serde_round_trip_socks5() {
        // Host uses RFC 5737 TEST-NET-3 (203.0.113.0/24) reserved for documentation.
        let json = r#"{
            "accountId": "a", "email": "e@t.com",
            "staticProxy": {
                "protocol": "socks5",
                "host": "203.0.113.10",
                "port": 45001,
                "username": "user1",
                "password": "p@ss"
            }
        }"#;
        let account: Account = serde_json::from_str(json).unwrap();
        let sp = account.static_proxy.as_ref().unwrap();
        assert_eq!(sp.protocol, "socks5");
        assert_eq!(sp.host, "203.0.113.10");
        assert_eq!(sp.port, 45001);
        assert_eq!(sp.username, "user1");
        assert_eq!(sp.password, "p@ss");

        let serialized = serde_json::to_string(&account).unwrap();
        assert!(serialized.contains("\"staticProxy\""));
        assert!(serialized.contains("\"protocol\":\"socks5\""));
        let restored: Account = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.static_proxy.as_ref().unwrap().port, 45001);
    }

    #[test]
    fn test_static_proxy_serde_round_trip_http() {
        let json = r#"{
            "accountId": "a", "email": "e@t.com",
            "staticProxy": {"protocol":"http","host":"h.example","port":8080,"username":"u","password":"p"}
        }"#;
        let account: Account = serde_json::from_str(json).unwrap();
        let sp = account.static_proxy.as_ref().unwrap();
        assert_eq!(sp.protocol, "http");
        assert_eq!(sp.port, 8080);
    }

    // ── list_accounts_in_dir ──────────────────────────────────

    fn create_v1_file(dir: &Path, account_id: &str, email: &str, created_at: i64) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}","fullName":"T","phoneNumber":"0",
            "accountUuid":"u","orgId":"o","sessionKey":"s","routingHint":"r",
            "device":{{"deviceId":"d","androidId":"a","platform":"android",
            "manufacturer":"G","model":"P","osVersion":"33","releaseVersion":"13",
            "locale":"en-US","timezone":"UTC","buildTags":"r","carrierName":"C",
            "carrierCountry":"us","appVersion":"1.0","anthropicDeviceId":"u"}},
            "region":"US","registrationId":"rg","createdAt":{}}}"#,
            account_id, email, created_at
        );
        write_file(dir, &format!("{}.json", account_id), &json);
    }

    #[test]
    fn test_list_empty_directory() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        let result = list_accounts_in_dir(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_list_nonexistent_directory() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        let result = list_accounts_in_dir(&dir.path().join("nope"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_list_multiple_sorted() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        create_v1_file(dir.path(), "ac_c", "c@t.com", 3000);
        create_v1_file(dir.path(), "ac_a", "a@t.com", 1000);
        create_v1_file(dir.path(), "ac_b", "b@t.com", 2000);
        let accounts = list_accounts_in_dir(dir.path()).unwrap();
        assert_eq!(accounts.len(), 3);
        assert_eq!(accounts[0].account_id, "ac_a");
        assert_eq!(accounts[2].account_id, "ac_c");
    }

    #[test]
    fn test_list_skips_corrupted() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        create_v1_file(dir.path(), "ac_good", "g@t.com", 1000);
        write_file(dir.path(), "bad.json", "not json");
        let accounts = list_accounts_in_dir(dir.path()).unwrap();
        assert_eq!(accounts.len(), 1);
    }

    #[test]
    fn test_list_skips_empty() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        create_v1_file(dir.path(), "ac_good", "g@t.com", 1000);
        write_file(dir.path(), "empty.json", "");
        assert_eq!(list_accounts_in_dir(dir.path()).unwrap().len(), 1);
    }

    #[test]
    fn test_list_ignores_non_json() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        create_v1_file(dir.path(), "ac_good", "g@t.com", 1000);
        write_file(dir.path(), "readme.md", "# test");
        write_file(dir.path(), ".DS_Store", "\0");
        assert_eq!(list_accounts_in_dir(dir.path()).unwrap().len(), 1);
    }

    #[test]
    fn test_list_all_corrupted_empty() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        write_file(dir.path(), "a.json", "garbage");
        write_file(dir.path(), "b.json", "");
        assert_eq!(list_accounts_in_dir(dir.path()).unwrap().len(), 0);
    }

    #[test]
    fn test_list_v3_json_loads() {
        let _g = TEST_MUTEX.lock().unwrap();
        let dir = TestDataDir::new();
        write_file(dir.path(), "v3.json", V3_JSON);
        let accounts = list_accounts_in_dir(dir.path()).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].subscription_type, "max");
    }

    // ── device_id tests ──────────────────────────────────

    #[test]
    fn test_generate_device_id_format() {
        let id = generate_device_id();
        assert_eq!(id.len(), 64, "device_id should be 64 hex chars");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "should be hex only");
    }

    #[test]
    fn test_generate_device_id_unique() {
        let id1 = generate_device_id();
        let id2 = generate_device_id();
        assert_ne!(id1, id2, "two generated device_ids should differ");
    }

    #[test]
    fn test_ensure_device_id_empty_generates() {
        let id = ensure_device_id("");
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_ensure_device_id_existing_preserved() {
        let existing = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let id = ensure_device_id(existing);
        assert_eq!(id, existing, "existing device_id should be returned as-is");
    }

    #[test]
    fn test_ensure_device_id_never_overwrites() {
        let first = ensure_device_id("");
        let second = ensure_device_id(&first);
        let third = ensure_device_id(&second);
        assert_eq!(first, second);
        assert_eq!(second, third);
    }

    #[test]
    fn test_cli_client_deserialize_with_device_id() {
        let json = r#"{
            "accessToken": "tok",
            "refreshToken": "ref",
            "expiresAt": 1000,
            "deviceId": "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344"
        }"#;
        let cli: CliClient = serde_json::from_str(json).unwrap();
        assert_eq!(cli.device_id, "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344");
    }

    #[test]
    fn test_cli_client_deserialize_without_device_id() {
        let json = r#"{
            "accessToken": "tok",
            "refreshToken": "ref",
            "expiresAt": 1000
        }"#;
        let cli: CliClient = serde_json::from_str(json).unwrap();
        assert!(cli.device_id.is_empty(), "missing deviceId should default to empty");
    }
}

// ─── Quota command ─────────────────────────────────────────

/// Fetch live usage data from /api/oauth/usage for an account.
/// Persists the result to Account JSON and emits a Tauri event.
#[tauri::command]
pub async fn get_account_quota(
    account_id: String,
    _app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<serde_json::Value, String> {
    let account = state.account_manager.read(&account_id).await
        .map_err(|e| format!("Account not found: {}", e))?;
    let cli = account.cli.as_ref()
        .ok_or("Account has no CLI credentials")?;

    // proxy_url: Direct mode returns None (no proxy)
    let proxy_url = state.proxy_allocator.get_proxy_url_optional(&account_id).await
        .map_err(|e| format!("get_proxy_url: {}", e))?;

    let boring = claude_ultra_http::BoringClient::builder().build()
        .map_err(|e| format!("BoringClient init failed: {}", e))?;

    let client = crate::modules::cli_client::CliClient::new(
        std::sync::Arc::new(boring),
        account_id.clone(),
        cli.access_token.clone(),
        cli.refresh_token.clone(),
        cli.expires_at,
        proxy_url,
    );

    let usage = match client.get_usage().await {
        Ok(u) => u,
        Err(crate::modules::cli_client::CliClientError::HttpStatus(status, ref msg))
            if matches!(status, 401 | 403) =>
        {
            let reason = format!("HTTP {}: {}", status, msg);
            let _ = state.account_manager.set_disabled(&account_id, Some(reason.clone())).await;
            return Err(reason);
        }
        Err(e) => return Err(format!("Failed to fetch usage: {}", e)),
    };
    let usage_value = serde_json::to_value(&usage)
        .map_err(|e| format!("Serialization error: {}", e))?;

    // Persist + notify consumers
    let snapshot = crate::models::quota::utilization_to_snapshot(&usage);
    state.account_manager.set_utilization(&account_id, usage_value.clone(), snapshot)
        .await.map_err(|e| format!("Failed to persist quota: {}", e))?;

    Ok(usage_value)
}
