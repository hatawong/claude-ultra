//! Security database — IP whitelist/blacklist + security config stored in gateway_logs.db.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::gateway_db::GatewayDb;

// ── Types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    Off,
    Whitelist,
    Blacklist,
}

impl Default for SecurityMode {
    fn default() -> Self {
        SecurityMode::Off
    }
}

impl SecurityMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecurityMode::Off => "off",
            SecurityMode::Whitelist => "whitelist",
            SecurityMode::Blacklist => "blacklist",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "whitelist" => SecurityMode::Whitelist,
            "blacklist" => SecurityMode::Blacklist,
            _ => SecurityMode::Off,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub mode: SecurityMode,
    pub auto_ban_enabled: bool,
    pub auto_ban_threshold: u32,
    pub auto_ban_duration_secs: i64,
    pub log_retention_days: u32,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            mode: SecurityMode::Off,
            auto_ban_enabled: false,
            auto_ban_threshold: 10,
            auto_ban_duration_secs: 3600,
            log_retention_days: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistEntry {
    pub id: i64,
    pub ip: String,
    pub description: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlacklistEntry {
    pub id: i64,
    pub ip: String,
    pub reason: Option<String>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogEntry {
    pub id: String,
    pub timestamp: i64,
    pub client_ip: Option<String>,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration_ms: u64,
    pub user_agent: Option<String>,
    pub api_key_prefix: Option<String>,
    pub model: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogResponse {
    pub logs: Vec<AccessLogEntry>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpStatsResponse {
    pub total_requests: i64,
    pub unique_ips: i64,
    pub blocked_requests: i64,
    pub top_ips: Vec<IpRanking>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRanking {
    pub client_ip: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub last_seen: i64,
}

// ── Schema Init ─────────────────────────────────────────────

/// Initialize security tables (call after GatewayDb::new).
pub fn init_security_tables(db: &GatewayDb) -> Result<(), String> {
    let conn = db.conn();

    // Add new columns to request_logs (ignore errors if already exist)
    for col in &[
        "ALTER TABLE request_logs ADD COLUMN client_ip TEXT",
        "ALTER TABLE request_logs ADD COLUMN user_agent TEXT",
        "ALTER TABLE request_logs ADD COLUMN api_key_prefix TEXT",
    ] {
        let _ = conn.execute(col, []);
    }

    // Index on client_ip
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_client_ip ON request_logs (client_ip);",
    )
    .map_err(|e| e.to_string())?;

    // Whitelist table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ip_whitelist (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ip TEXT NOT NULL UNIQUE,
            description TEXT,
            created_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;

    // Blacklist table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ip_blacklist (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ip TEXT NOT NULL UNIQUE,
            reason TEXT,
            expires_at INTEGER,
            created_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;

    // Security config table (single-row)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS security_config (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL DEFAULT 'off',
            auto_ban_enabled INTEGER NOT NULL DEFAULT 0,
            auto_ban_threshold INTEGER NOT NULL DEFAULT 10,
            auto_ban_duration_secs INTEGER NOT NULL DEFAULT 3600,
            log_retention_days INTEGER NOT NULL DEFAULT 30
        );
        INSERT OR IGNORE INTO security_config (id) VALUES (1);",
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

// ── Security Config CRUD ────────────────────────────────────

pub fn get_security_config(db: &GatewayDb) -> Result<SecurityConfig, String> {
    let conn = db.conn();
    conn.query_row(
        "SELECT mode, auto_ban_enabled, auto_ban_threshold, auto_ban_duration_secs, log_retention_days
         FROM security_config WHERE id = 1",
        [],
        |row| {
            let mode_str: String = row.get(0)?;
            Ok(SecurityConfig {
                mode: SecurityMode::from_str(&mode_str),
                auto_ban_enabled: row.get::<_, i32>(1)? != 0,
                auto_ban_threshold: row.get::<_, i32>(2)? as u32,
                auto_ban_duration_secs: row.get(3)?,
                log_retention_days: row.get::<_, i32>(4)? as u32,
            })
        },
    )
    .map_err(|e| e.to_string())
}

pub fn update_security_config(db: &GatewayDb, config: &SecurityConfig) -> Result<(), String> {
    let conn = db.conn();
    conn.execute(
        "UPDATE security_config SET mode = ?1, auto_ban_enabled = ?2,
         auto_ban_threshold = ?3, auto_ban_duration_secs = ?4, log_retention_days = ?5
         WHERE id = 1",
        params![
            config.mode.as_str(),
            config.auto_ban_enabled as i32,
            config.auto_ban_threshold as i32,
            config.auto_ban_duration_secs,
            config.log_retention_days as i32,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Whitelist CRUD ──────────────────────────────────────────

pub fn list_whitelist(db: &GatewayDb) -> Result<Vec<WhitelistEntry>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, ip, description, created_at FROM ip_whitelist ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WhitelistEntry {
                id: row.get(0)?,
                ip: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn add_whitelist(db: &GatewayDb, ip: &str, description: Option<&str>) -> Result<i64, String> {
    let conn = db.conn();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "INSERT INTO ip_whitelist (ip, description, created_at) VALUES (?1, ?2, ?3)",
        params![ip, description, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub fn remove_whitelist(db: &GatewayDb, id: i64) -> Result<bool, String> {
    let conn = db.conn();
    let deleted = conn
        .execute("DELETE FROM ip_whitelist WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(deleted > 0)
}

pub fn is_whitelisted(db: &GatewayDb, ip: &str) -> Result<bool, String> {
    let conn = db.conn();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ip_whitelist WHERE ip = ?1",
            params![ip],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

// ── Blacklist CRUD ──────────────────────────────────────────

pub fn list_blacklist(db: &GatewayDb) -> Result<Vec<BlacklistEntry>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, ip, reason, expires_at, created_at FROM ip_blacklist ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BlacklistEntry {
                id: row.get(0)?,
                ip: row.get(1)?,
                reason: row.get(2)?,
                expires_at: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn add_blacklist(
    db: &GatewayDb,
    ip: &str,
    reason: Option<&str>,
    expires_at: Option<i64>,
) -> Result<i64, String> {
    let conn = db.conn();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "INSERT INTO ip_blacklist (ip, reason, expires_at, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![ip, reason, expires_at, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub fn remove_blacklist(db: &GatewayDb, id: i64) -> Result<bool, String> {
    let conn = db.conn();
    let deleted = conn
        .execute("DELETE FROM ip_blacklist WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(deleted > 0)
}

pub fn is_blacklisted(db: &GatewayDb, ip: &str) -> Result<bool, String> {
    let conn = db.conn();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ip_blacklist WHERE ip = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
            params![ip, now],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

// ── Access Logs (query request_logs with client_ip) ─────────

pub fn get_access_logs(
    db: &GatewayDb,
    limit: usize,
    offset: usize,
    client_ip: Option<&str>,
    search: Option<&str>,
) -> Result<AccessLogResponse, String> {
    let conn = db.conn();

    let mut where_clauses = vec!["1=1".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ip) = client_ip {
        where_clauses.push("client_ip = ?".to_string());
        param_values.push(Box::new(ip.to_string()));
    }
    if let Some(s) = search {
        where_clauses.push("(client_ip LIKE ? OR url LIKE ? OR user_agent LIKE ?)".to_string());
        let pattern = format!("%{}%", s);
        param_values.push(Box::new(pattern.clone()));
        param_values.push(Box::new(pattern.clone()));
        param_values.push(Box::new(pattern));
    }

    let where_sql = where_clauses.join(" AND ");

    // Count total
    let count_sql = format!("SELECT COUNT(*) FROM request_logs WHERE {}", where_sql);
    let count_params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
    let total: i64 = conn
        .query_row(&count_sql, count_params.as_slice(), |row| row.get(0))
        .map_err(|e| e.to_string())?;

    // Query logs
    let query_sql = format!(
        "SELECT id, timestamp, client_ip, method, url, status, duration_ms,
         user_agent, api_key_prefix, model, account_id
         FROM request_logs WHERE {} ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        where_sql
    );
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ip) = client_ip {
        all_params.push(Box::new(ip.to_string()));
    }
    if let Some(s) = search {
        let pattern = format!("%{}%", s);
        all_params.push(Box::new(pattern.clone()));
        all_params.push(Box::new(pattern.clone()));
        all_params.push(Box::new(pattern));
    }
    all_params.push(Box::new(limit as i64));
    all_params.push(Box::new(offset as i64));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&query_sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(AccessLogEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                client_ip: row.get(2)?,
                method: row.get(3)?,
                url: row.get(4)?,
                status: row.get::<_, i32>(5)? as u16,
                duration_ms: row.get::<_, i64>(6)? as u64,
                user_agent: row.get(7)?,
                api_key_prefix: row.get(8)?,
                model: row.get(9)?,
                account_id: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let logs: Vec<AccessLogEntry> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok(AccessLogResponse { logs, total })
}

// ── IP Statistics ───────────────────────────────────────────

pub fn get_ip_statistics(db: &GatewayDb, hours: i64) -> Result<IpStatsResponse, String> {
    let conn = db.conn();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let since_ms = now_ms - (hours * 3600 * 1000);

    let (total_requests, unique_ips): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT client_ip)
             FROM request_logs WHERE timestamp >= ?1 AND client_ip IS NOT NULL",
            params![since_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;

    let blocked_requests: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM request_logs WHERE timestamp >= ?1 AND status = 403",
            params![since_ms],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT client_ip, COUNT(*) as cnt,
             COALESCE(SUM(total_tokens), 0),
             COALESCE(SUM(input_tokens), 0),
             COALESCE(SUM(output_tokens), 0),
             MAX(timestamp)
             FROM request_logs
             WHERE timestamp >= ?1 AND client_ip IS NOT NULL
             GROUP BY client_ip
             ORDER BY cnt DESC
             LIMIT 20",
        )
        .map_err(|e| e.to_string())?;

    let top_ips = stmt
        .query_map(params![since_ms], |row| {
            Ok(IpRanking {
                client_ip: row.get(0)?,
                request_count: row.get(1)?,
                total_tokens: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                last_seen: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(IpStatsResponse {
        total_requests,
        unique_ips,
        blocked_requests,
        top_ips,
    })
}

// ── Cleanup ─────────────────────────────────────────────────

/// Remove expired blacklist entries.
pub fn cleanup_expired_blacklist(db: &GatewayDb) -> Result<usize, String> {
    let conn = db.conn();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let deleted = conn
        .execute(
            "DELETE FROM ip_blacklist WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            params![now],
        )
        .map_err(|e| e.to_string())?;
    Ok(deleted)
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn make_test_db() -> Arc<GatewayDb> {
        let path = std::env::temp_dir().join(format!(
            "claude_ultra_security_db_test_{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(GatewayDb::new(&path).unwrap());
        init_security_tables(&db).unwrap();
        db
    }

    // ── SecurityConfig tests ────────────────────────────────

    #[test]
    fn test_default_security_config() {
        let db = make_test_db();
        let config = get_security_config(&db).unwrap();
        assert_eq!(config.mode, SecurityMode::Off);
        assert!(!config.auto_ban_enabled);
        assert_eq!(config.auto_ban_threshold, 10);
        assert_eq!(config.auto_ban_duration_secs, 3600);
        assert_eq!(config.log_retention_days, 30);
    }

    #[test]
    fn test_update_security_config() {
        let db = make_test_db();
        let config = SecurityConfig {
            mode: SecurityMode::Whitelist,
            auto_ban_enabled: true,
            auto_ban_threshold: 5,
            auto_ban_duration_secs: 7200,
            log_retention_days: 14,
        };
        update_security_config(&db, &config).unwrap();

        let loaded = get_security_config(&db).unwrap();
        assert_eq!(loaded.mode, SecurityMode::Whitelist);
        assert!(loaded.auto_ban_enabled);
        assert_eq!(loaded.auto_ban_threshold, 5);
        assert_eq!(loaded.auto_ban_duration_secs, 7200);
        assert_eq!(loaded.log_retention_days, 14);
    }

    #[test]
    fn test_security_mode_roundtrip() {
        for mode in &[SecurityMode::Off, SecurityMode::Whitelist, SecurityMode::Blacklist] {
            let s = mode.as_str();
            assert_eq!(SecurityMode::from_str(s), *mode);
        }
    }

    // ── Whitelist tests ─────────────────────────────────────

    #[test]
    fn test_whitelist_add_and_list() {
        let db = make_test_db();
        add_whitelist(&db, "192.168.1.100", Some("home mac")).unwrap();
        add_whitelist(&db, "127.0.0.1", Some("localhost")).unwrap();

        let list = list_whitelist(&db).unwrap();
        assert_eq!(list.len(), 2);
        let ips: Vec<&str> = list.iter().map(|e| e.ip.as_str()).collect();
        assert!(ips.contains(&"192.168.1.100"));
        assert!(ips.contains(&"127.0.0.1"));
    }

    #[test]
    fn test_whitelist_remove() {
        let db = make_test_db();
        let id = add_whitelist(&db, "10.0.0.1", None).unwrap();
        assert!(remove_whitelist(&db, id).unwrap());
        assert!(list_whitelist(&db).unwrap().is_empty());
    }

    #[test]
    fn test_whitelist_duplicate_rejected() {
        let db = make_test_db();
        add_whitelist(&db, "192.168.1.1", None).unwrap();
        let result = add_whitelist(&db, "192.168.1.1", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_whitelisted() {
        let db = make_test_db();
        add_whitelist(&db, "192.168.1.1", None).unwrap();
        assert!(is_whitelisted(&db, "192.168.1.1").unwrap());
        assert!(!is_whitelisted(&db, "10.0.0.1").unwrap());
    }

    // ── Blacklist tests ─────────────────────────────────────

    #[test]
    fn test_blacklist_add_and_list() {
        let db = make_test_db();
        add_blacklist(&db, "1.2.3.4", Some("spam"), None).unwrap();
        let list = list_blacklist(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ip, "1.2.3.4");
        assert_eq!(list[0].reason.as_deref(), Some("spam"));
    }

    #[test]
    fn test_blacklist_remove() {
        let db = make_test_db();
        let id = add_blacklist(&db, "5.6.7.8", None, None).unwrap();
        assert!(remove_blacklist(&db, id).unwrap());
        assert!(list_blacklist(&db).unwrap().is_empty());
    }

    #[test]
    fn test_blacklist_duplicate_rejected() {
        let db = make_test_db();
        add_blacklist(&db, "9.8.7.6", None, None).unwrap();
        let result = add_blacklist(&db, "9.8.7.6", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_blacklisted_permanent() {
        let db = make_test_db();
        add_blacklist(&db, "1.1.1.1", None, None).unwrap();
        assert!(is_blacklisted(&db, "1.1.1.1").unwrap());
        assert!(!is_blacklisted(&db, "2.2.2.2").unwrap());
    }

    #[test]
    fn test_is_blacklisted_expired() {
        let db = make_test_db();
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 1;
        add_blacklist(&db, "3.3.3.3", None, Some(past)).unwrap();
        assert!(!is_blacklisted(&db, "3.3.3.3").unwrap());
    }

    #[test]
    fn test_is_blacklisted_not_expired() {
        let db = make_test_db();
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        add_blacklist(&db, "4.4.4.4", None, Some(future)).unwrap();
        assert!(is_blacklisted(&db, "4.4.4.4").unwrap());
    }

    #[test]
    fn test_cleanup_expired_blacklist() {
        let db = make_test_db();
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 1;
        let future = past + 7200;
        add_blacklist(&db, "expired.ip", None, Some(past)).unwrap();
        add_blacklist(&db, "active.ip", None, Some(future)).unwrap();
        add_blacklist(&db, "permanent.ip", None, None).unwrap();

        let deleted = cleanup_expired_blacklist(&db).unwrap();
        assert_eq!(deleted, 1);

        let remaining = list_blacklist(&db).unwrap();
        assert_eq!(remaining.len(), 2);
    }

    // ── Access logs tests ───────────────────────────────────

    #[test]
    fn test_access_logs_empty() {
        let db = make_test_db();
        let resp = get_access_logs(&db, 50, 0, None, None).unwrap();
        assert_eq!(resp.total, 0);
        assert!(resp.logs.is_empty());
    }

    #[test]
    fn test_access_logs_with_client_ip() {
        let db = make_test_db();
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO request_logs (id, timestamp, method, url, status, duration_ms, client_ip, user_agent, api_key_prefix)
                 VALUES ('log1', 1000, 'POST', '/v1/messages', 200, 500, '192.168.1.5', 'curl/8.0', 'sk-ultra')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO request_logs (id, timestamp, method, url, status, duration_ms, client_ip)
                 VALUES ('log2', 1001, 'POST', '/v1/messages', 200, 300, '10.0.0.1')",
                [],
            ).unwrap();
        }

        let resp = get_access_logs(&db, 50, 0, Some("192.168.1.5"), None).unwrap();
        assert_eq!(resp.total, 1);
        assert_eq!(resp.logs[0].client_ip.as_deref(), Some("192.168.1.5"));
        assert_eq!(resp.logs[0].user_agent.as_deref(), Some("curl/8.0"));

        let resp = get_access_logs(&db, 50, 0, None, None).unwrap();
        assert_eq!(resp.total, 2);
    }

    // ── IP Statistics tests ─────────────────────────────────

    #[test]
    fn test_ip_statistics_empty() {
        let db = make_test_db();
        let stats = get_ip_statistics(&db, 24).unwrap();
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.unique_ips, 0);
        assert_eq!(stats.blocked_requests, 0);
        assert!(stats.top_ips.is_empty());
    }
}
