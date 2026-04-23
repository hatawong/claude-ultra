//! Gateway request log database — SQLite WAL storage for request logs.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// A single gateway request log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    pub id: String,
    pub timestamp: i64,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration_ms: u64,
    pub model: Option<String>,
    pub account_id: Option<String>,
    pub account_email: Option<String>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub input_cost: Option<f64>,
    pub output_cost: Option<f64>,
    pub cache_creation_cost: Option<f64>,
    pub cache_read_cost: Option<f64>,
    pub total_cost: Option<f64>,
    pub error: Option<String>,
    pub request_size: Option<u64>,
    pub response_size: Option<u64>,
    // Security fields (Round 9)
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub api_key_prefix: Option<String>,
    // Body fields (Round 21)
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    // Headers as JSON array: [["name","value"], ...]
    pub request_headers: Option<String>,
    pub response_headers: Option<String>,
}

/// Aggregated token stats for a time period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStatsRow {
    pub period: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub request_count: i64,
}

/// Cost summary over a date range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub by_model: Vec<ModelCostRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCostRow {
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
}

/// Get database path.
pub fn get_db_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude-ultra").join("gateway_logs.db"))
}

/// Shared SQLite connection for gateway + security tables.
pub struct GatewayDb {
    conn: Mutex<Connection>,
}

impl GatewayDb {
    /// Open database and initialize schema.
    pub fn new(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| e.to_string())?;

        // Create request_logs table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS request_logs (
                id TEXT PRIMARY KEY,
                timestamp INTEGER NOT NULL,
                method TEXT,
                url TEXT,
                status INTEGER,
                duration_ms INTEGER,
                model TEXT,
                account_id TEXT,
                account_email TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_creation_tokens INTEGER,
                cache_read_tokens INTEGER,
                total_tokens INTEGER,
                input_cost REAL,
                output_cost REAL,
                cache_creation_cost REAL,
                cache_read_cost REAL,
                total_cost REAL,
                error TEXT,
                request_size INTEGER,
                response_size INTEGER,
                client_ip TEXT,
                user_agent TEXT,
                api_key_prefix TEXT,
                request_body TEXT,
                response_body TEXT,
                request_headers TEXT,
                response_headers TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_timestamp ON request_logs (timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_account_id ON request_logs (account_id);
            CREATE INDEX IF NOT EXISTS idx_model ON request_logs (model);
            CREATE INDEX IF NOT EXISTS idx_client_ip ON request_logs (client_ip);",
        )
        .map_err(|e| e.to_string())?;

        // Column migrations: each ALTER runs independently and tolerates
        // the "duplicate column name" error so a partial-migration state
        // (one column present, sibling missing) still converges.
        for stmt in [
            "ALTER TABLE request_logs ADD COLUMN request_body TEXT",
            "ALTER TABLE request_logs ADD COLUMN response_body TEXT",
            "ALTER TABLE request_logs ADD COLUMN request_headers TEXT",
            "ALTER TABLE request_logs ADD COLUMN response_headers TEXT",
        ] {
            if let Err(e) = conn.execute(stmt, []) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(format!("Migration failed: {}", msg));
                }
            }
        }

        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Get a lock on the connection. Used by security_db.
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    /// Save a request log entry.
    pub fn save_log(&self, log: &RequestLog) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO request_logs (id, timestamp, method, url, status, duration_ms, model,
             account_id, account_email, input_tokens, output_tokens, cache_creation_tokens,
             cache_read_tokens, total_tokens, input_cost, output_cost, cache_creation_cost,
             cache_read_cost, total_cost, error, request_size, response_size,
             client_ip, user_agent, api_key_prefix, request_body, response_body,
             request_headers, response_headers)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)
             ON CONFLICT(id) DO UPDATE SET
                timestamp=excluded.timestamp,
                status=excluded.status,
                duration_ms=excluded.duration_ms,
                account_id=excluded.account_id,
                account_email=excluded.account_email,
                request_size=excluded.request_size,
                response_size=excluded.response_size,
                client_ip=excluded.client_ip,
                user_agent=excluded.user_agent,
                api_key_prefix=excluded.api_key_prefix,
                request_body=excluded.request_body,
                response_body=excluded.response_body,
                request_headers=excluded.request_headers,
                response_headers=excluded.response_headers,
                error=excluded.error,
                input_tokens=excluded.input_tokens,
                output_tokens=excluded.output_tokens,
                cache_creation_tokens=excluded.cache_creation_tokens,
                cache_read_tokens=excluded.cache_read_tokens,
                total_tokens=excluded.total_tokens,
                input_cost=excluded.input_cost,
                output_cost=excluded.output_cost,
                cache_creation_cost=excluded.cache_creation_cost,
                cache_read_cost=excluded.cache_read_cost,
                total_cost=excluded.total_cost",
            params![
                log.id,
                log.timestamp,
                log.method,
                log.url,
                log.status,
                log.duration_ms,
                log.model,
                log.account_id,
                log.account_email,
                log.input_tokens,
                log.output_tokens,
                log.cache_creation_tokens,
                log.cache_read_tokens,
                log.total_tokens,
                log.input_cost,
                log.output_cost,
                log.cache_creation_cost,
                log.cache_read_cost,
                log.total_cost,
                log.error,
                log.request_size,
                log.response_size,
                log.client_ip,
                log.user_agent,
                log.api_key_prefix,
                log.request_body,
                log.response_body,
                log.request_headers,
                log.response_headers,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Get logs with pagination and optional filters.
    pub fn get_logs(
        &self,
        limit: usize,
        offset: usize,
        account_id: Option<&str>,
        model: Option<&str>,
        date_from: Option<i64>,
        date_to: Option<i64>,
        search: Option<&str>,
    ) -> Result<Vec<RequestLog>, String> {
        let conn = self.conn();

        let mut sql = String::from(
            "SELECT id, timestamp, method, url, status, duration_ms, model,
             account_id, account_email, input_tokens, output_tokens,
             cache_creation_tokens, cache_read_tokens, total_tokens,
             input_cost, output_cost, cache_creation_cost, cache_read_cost, total_cost,
             error, request_size, response_size, client_ip, user_agent, api_key_prefix
             FROM request_logs WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = search {
            if !s.is_empty() {
                let pattern = format!("%{}%", s);
                sql.push_str(" AND (model LIKE ? OR account_email LIKE ? OR error LIKE ?)");
                param_values.push(Box::new(pattern.clone()));
                param_values.push(Box::new(pattern.clone()));
                param_values.push(Box::new(pattern));
            }
        }
        if let Some(aid) = account_id {
            sql.push_str(" AND account_id = ?");
            param_values.push(Box::new(aid.to_string()));
        }
        if let Some(m) = model {
            sql.push_str(" AND model = ?");
            param_values.push(Box::new(m.to_string()));
        }
        if let Some(from) = date_from {
            sql.push_str(" AND timestamp >= ?");
            param_values.push(Box::new(from));
        }
        if let Some(to) = date_to {
            sql.push_str(" AND timestamp <= ?");
            param_values.push(Box::new(to));
        }

        sql.push_str(" ORDER BY timestamp DESC LIMIT ? OFFSET ?");
        param_values.push(Box::new(limit as i64));
        param_values.push(Box::new(offset as i64));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(RequestLog {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    method: row.get(2)?,
                    url: row.get(3)?,
                    status: row.get::<_, i32>(4)? as u16,
                    duration_ms: row.get::<_, i64>(5)? as u64,
                    model: row.get(6)?,
                    account_id: row.get(7)?,
                    account_email: row.get(8)?,
                    input_tokens: row.get::<_, Option<i32>>(9)?.map(|v| v as u32),
                    output_tokens: row.get::<_, Option<i32>>(10)?.map(|v| v as u32),
                    cache_creation_tokens: row.get::<_, Option<i32>>(11)?.map(|v| v as u32),
                    cache_read_tokens: row.get::<_, Option<i32>>(12)?.map(|v| v as u32),
                    total_tokens: row.get::<_, Option<i32>>(13)?.map(|v| v as u32),
                    input_cost: row.get(14)?,
                    output_cost: row.get(15)?,
                    cache_creation_cost: row.get(16)?,
                    cache_read_cost: row.get(17)?,
                    total_cost: row.get(18)?,
                    error: row.get(19)?,
                    request_size: row.get::<_, Option<i64>>(20)?.map(|v| v as u64),
                    response_size: row.get::<_, Option<i64>>(21)?.map(|v| v as u64),
                    client_ip: row.get(22)?,
                    user_agent: row.get(23)?,
                    api_key_prefix: row.get(24)?,
                    request_body: None,  // Not loaded in list view (use get_log_detail)
                    response_body: None,
                    request_headers: None,
                    response_headers: None,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(row.map_err(|e| e.to_string())?);
        }
        Ok(logs)
    }

    /// Get token stats aggregated by time period.
    pub fn get_token_stats(
        &self,
        period: &str,
        account_id: Option<&str>,
    ) -> Result<Vec<TokenStatsRow>, String> {
        let conn = self.conn();

        let group_expr = match period {
            "hour" => "strftime('%Y-%m-%d %H:00', timestamp/1000, 'unixepoch', 'localtime')",
            "day" => "strftime('%Y-%m-%d', timestamp/1000, 'unixepoch', 'localtime')",
            "week" => "strftime('%Y-W%W', timestamp/1000, 'unixepoch', 'localtime')",
            _ => return Err(format!("Invalid period: {}", period)),
        };

        let mut sql = format!(
            "SELECT {group} as period,
             COALESCE(SUM(input_tokens), 0),
             COALESCE(SUM(output_tokens), 0),
             COALESCE(SUM(total_tokens), 0),
             COALESCE(SUM(total_cost), 0.0),
             COUNT(*)
             FROM request_logs WHERE 1=1",
            group = group_expr
        );

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(aid) = account_id {
            sql.push_str(" AND account_id = ?");
            param_values.push(Box::new(aid.to_string()));
        }
        sql.push_str(&format!(" GROUP BY {} ORDER BY period", group_expr));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(TokenStatsRow {
                    period: row.get(0)?,
                    total_input_tokens: row.get(1)?,
                    total_output_tokens: row.get(2)?,
                    total_tokens: row.get(3)?,
                    total_cost: row.get(4)?,
                    request_count: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut stats = Vec::new();
        for row in rows {
            stats.push(row.map_err(|e| e.to_string())?);
        }
        Ok(stats)
    }

    /// Get cost summary for a date range.
    pub fn get_cost_summary(
        &self,
        date_from: i64,
        date_to: i64,
    ) -> Result<CostSummary, String> {
        let conn = self.conn();

        let (total_requests, total_input, total_output, total_tokens, total_cost): (i64, i64, i64, i64, f64) = conn
            .query_row(
                "SELECT COUNT(*),
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(total_tokens), 0),
                 COALESCE(SUM(total_cost), 0.0)
                 FROM request_logs WHERE timestamp >= ?1 AND timestamp <= ?2",
                params![date_from, date_to],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT model, COUNT(*), COALESCE(SUM(total_tokens), 0), COALESCE(SUM(total_cost), 0.0)
                 FROM request_logs WHERE timestamp >= ?1 AND timestamp <= ?2
                 GROUP BY model ORDER BY SUM(total_cost) DESC",
            )
            .map_err(|e| e.to_string())?;

        let model_rows = stmt
            .query_map(params![date_from, date_to], |row| {
                Ok(ModelCostRow {
                    model: row.get::<_, Option<String>>(0)?.unwrap_or_else(|| "unknown".to_string()),
                    request_count: row.get(1)?,
                    total_tokens: row.get(2)?,
                    total_cost: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut by_model = Vec::new();
        for row in model_rows {
            by_model.push(row.map_err(|e| e.to_string())?);
        }

        Ok(CostSummary {
            total_requests,
            total_input_tokens: total_input,
            total_output_tokens: total_output,
            total_tokens,
            total_cost,
            by_model,
        })
    }

    /// Clear logs optionally before a timestamp.
    pub fn clear_logs(&self, before_timestamp: Option<i64>) -> Result<usize, String> {
        let conn = self.conn();
        let deleted = if let Some(ts) = before_timestamp {
            conn.execute("DELETE FROM request_logs WHERE timestamp < ?1", params![ts])
        } else {
            conn.execute("DELETE FROM request_logs", [])
        }
        .map_err(|e| e.to_string())?;
        Ok(deleted)
    }

    /// Update only the token usage and cost fields on an existing log entry.
    pub fn update_usage(
        &self,
        log_id: &str,
        usage: &super::billing::TokenUsage,
        cost: &super::billing::CostBreakdown,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE request_logs SET
             input_tokens=?1, output_tokens=?2,
             cache_creation_tokens=?3, cache_read_tokens=?4, total_tokens=?5,
             input_cost=?6, output_cost=?7,
             cache_creation_cost=?8, cache_read_cost=?9, total_cost=?10
             WHERE id=?11",
            params![
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_creation_tokens,
                usage.cache_read_tokens,
                usage.total_tokens(),
                cost.input_cost,
                cost.output_cost,
                cost.cache_creation_cost,
                cost.cache_read_cost,
                cost.total_cost,
                log_id,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Update response_body for a log entry.
    pub fn update_response_body(&self, log_id: &str, body: &str) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE request_logs SET response_body=?1, response_size=?2 WHERE id=?3",
            params![body, body.len() as i64, log_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Update response_headers for a log entry (used by SSE path after first chunk).
    pub fn update_response_headers(&self, log_id: &str, headers_json: &str) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE request_logs SET response_headers=?1 WHERE id=?2",
            params![headers_json, log_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Revise status + error for a previously written log row. Used when a
    /// provisional 200 row was written before streaming began but the stream
    /// later failed and the client actually received a 5xx.
    pub fn update_status_and_error(
        &self,
        log_id: &str,
        status: u16,
        error: &str,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE request_logs SET status=?1, error=?2 WHERE id=?3",
            params![status as i64, error, log_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Get total count of logs matching optional search filter.
    pub fn get_logs_count(&self, search: Option<&str>) -> Result<u64, String> {
        let conn = self.conn();

        let (sql, param_values): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match search {
            Some(s) if !s.is_empty() => {
                let pattern = format!("%{}%", s);
                (
                    "SELECT COUNT(*) FROM request_logs WHERE model LIKE ?1 OR account_email LIKE ?1 OR error LIKE ?1",
                    vec![Box::new(pattern) as Box<dyn rusqlite::types::ToSql>],
                )
            }
            _ => ("SELECT COUNT(*) FROM request_logs", vec![]),
        };

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
        let count: u64 = conn
            .query_row(sql, params_refs.as_slice(), |row| row.get(0))
            .map_err(|e| e.to_string())?;
        Ok(count)
    }

    /// Get a single log entry with full body content.
    pub fn get_log_detail(&self, id: &str) -> Result<RequestLog, String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, timestamp, method, url, status, duration_ms, model,
             account_id, account_email, input_tokens, output_tokens,
             cache_creation_tokens, cache_read_tokens, total_tokens,
             input_cost, output_cost, cache_creation_cost, cache_read_cost, total_cost,
             error, request_size, response_size, client_ip, user_agent, api_key_prefix,
             request_body, response_body, request_headers, response_headers
             FROM request_logs WHERE id = ?1",
            params![id],
            |row| {
                Ok(RequestLog {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    method: row.get(2)?,
                    url: row.get(3)?,
                    status: row.get::<_, i32>(4)? as u16,
                    duration_ms: row.get::<_, i64>(5)? as u64,
                    model: row.get(6)?,
                    account_id: row.get(7)?,
                    account_email: row.get(8)?,
                    input_tokens: row.get::<_, Option<i32>>(9)?.map(|v| v as u32),
                    output_tokens: row.get::<_, Option<i32>>(10)?.map(|v| v as u32),
                    cache_creation_tokens: row.get::<_, Option<i32>>(11)?.map(|v| v as u32),
                    cache_read_tokens: row.get::<_, Option<i32>>(12)?.map(|v| v as u32),
                    total_tokens: row.get::<_, Option<i32>>(13)?.map(|v| v as u32),
                    input_cost: row.get(14)?,
                    output_cost: row.get(15)?,
                    cache_creation_cost: row.get(16)?,
                    cache_read_cost: row.get(17)?,
                    total_cost: row.get(18)?,
                    error: row.get(19)?,
                    request_size: row.get::<_, Option<i64>>(20)?.map(|v| v as u64),
                    response_size: row.get::<_, Option<i64>>(21)?.map(|v| v as u64),
                    client_ip: row.get(22)?,
                    user_agent: row.get(23)?,
                    api_key_prefix: row.get(24)?,
                    request_body: row.get(25)?,
                    response_body: row.get(26)?,
                    request_headers: row.get(27)?,
                    response_headers: row.get(28)?,
                })
            },
        )
        .map_err(|e| e.to_string())
    }
}

// ── Tauri IPC Commands ────────────────────────────────────

#[tauri::command]
pub async fn get_request_logs(
    db: tauri::State<'_, std::sync::Arc<GatewayDb>>,
    limit: usize,
    offset: usize,
    account_id: Option<String>,
    model: Option<String>,
    date_from: Option<i64>,
    date_to: Option<i64>,
    search: Option<String>,
) -> Result<Vec<RequestLog>, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || {
        db.get_logs(
            limit,
            offset,
            account_id.as_deref(),
            model.as_deref(),
            date_from,
            date_to,
            search.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_token_stats(
    db: tauri::State<'_, std::sync::Arc<GatewayDb>>,
    period: String,
    account_id: Option<String>,
) -> Result<Vec<TokenStatsRow>, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || {
        db.get_token_stats(&period, account_id.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_cost_summary(
    db: tauri::State<'_, std::sync::Arc<GatewayDb>>,
    date_from: i64,
    date_to: i64,
) -> Result<CostSummary, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || db.get_cost_summary(date_from, date_to))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_logs_count(
    db: tauri::State<'_, std::sync::Arc<GatewayDb>>,
    search: Option<String>,
) -> Result<u64, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || db.get_logs_count(search.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clear_gateway_logs(
    db: tauri::State<'_, std::sync::Arc<GatewayDb>>,
    before_timestamp: Option<i64>,
) -> Result<usize, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || db.clear_logs(before_timestamp))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_log_detail(
    db: tauri::State<'_, std::sync::Arc<GatewayDb>>,
    id: String,
) -> Result<RequestLog, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || db.get_log_detail(&id))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn make_test_db() -> Arc<GatewayDb> {
        let path = std::env::temp_dir().join(format!(
            "claude_ultra_gateway_db_test_{}.db",
            uuid::Uuid::new_v4()
        ));
        Arc::new(GatewayDb::new(&path).unwrap())
    }

    /// Build a legacy schema at the given path so migrations must run on open.
    fn make_partial_schema(path: &std::path::Path, columns: &[&str]) {
        use rusqlite::Connection;
        let conn = Connection::open(path).unwrap();
        let base = "CREATE TABLE request_logs (
            id TEXT PRIMARY KEY,
            timestamp INTEGER NOT NULL,
            method TEXT,
            url TEXT,
            status INTEGER,
            duration_ms INTEGER,
            model TEXT,
            account_id TEXT,
            account_email TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_creation_tokens INTEGER,
            cache_read_tokens INTEGER,
            total_tokens INTEGER,
            input_cost REAL,
            output_cost REAL,
            cache_creation_cost REAL,
            cache_read_cost REAL,
            total_cost REAL,
            error TEXT,
            request_size INTEGER,
            response_size INTEGER,
            client_ip TEXT,
            user_agent TEXT,
            api_key_prefix TEXT";
        let extra: String = columns
            .iter()
            .map(|c| format!(", {} TEXT", c))
            .collect::<Vec<_>>()
            .join("");
        let sql = format!("{}{});", base, extra);
        conn.execute_batch(&sql).unwrap();
    }

    #[test]
    fn test_migration_only_request_body_present() {
        // Legacy schema already has request_body but is missing the other three.
        // Batch migrations would ADD-duplicate-fail on request_body; the new
        // per-column loop tolerates the duplicate and still adds the siblings.
        let path = std::env::temp_dir().join(format!(
            "claude_ultra_gateway_db_partial_{}.db",
            uuid::Uuid::new_v4()
        ));
        make_partial_schema(&path, &["request_body"]);
        let db = GatewayDb::new(&path).expect("new() must tolerate partial migration");
        let log = make_log("r1", "claude-sonnet-4-6", "acc1", 1000);
        db.save_log(&log).expect("save_log must work after migration");
    }

    #[test]
    fn test_migration_only_response_headers_present() {
        let path = std::env::temp_dir().join(format!(
            "claude_ultra_gateway_db_partial_{}.db",
            uuid::Uuid::new_v4()
        ));
        make_partial_schema(&path, &["response_headers"]);
        let db = GatewayDb::new(&path).expect("new() must tolerate partial migration");
        let log = make_log("r1", "claude-sonnet-4-6", "acc1", 1000);
        db.save_log(&log).expect("save_log must work after migration");
    }

    #[test]
    fn test_migration_idempotent_on_fully_migrated_db() {
        // Running new() twice on the same path must not fail.
        let path = std::env::temp_dir().join(format!(
            "claude_ultra_gateway_db_idem_{}.db",
            uuid::Uuid::new_v4()
        ));
        let _ = GatewayDb::new(&path).unwrap();
        let _ = GatewayDb::new(&path).unwrap();
    }

    fn make_log(id: &str, model: &str, account_id: &str, timestamp: i64) -> RequestLog {
        RequestLog {
            id: id.to_string(),
            timestamp,
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            status: 200,
            duration_ms: 1500,
            model: Some(model.to_string()),
            account_id: Some(account_id.to_string()),
            account_email: Some(format!("{}@test.com", account_id)),
            input_tokens: Some(1000),
            output_tokens: Some(500),
            cache_creation_tokens: Some(100),
            cache_read_tokens: Some(200),
            total_tokens: Some(1800),
            input_cost: Some(0.003),
            output_cost: Some(0.0075),
            cache_creation_cost: Some(0.000375),
            cache_read_cost: Some(0.00006),
            total_cost: Some(0.010935),
            error: None,
            request_size: Some(5000),
            response_size: Some(2000),
            client_ip: None,
            user_agent: None,
            api_key_prefix: None,
            request_body: None,
            response_body: None,
            request_headers: None,
            response_headers: None,
        }
    }

    #[test]
    fn test_init_db_creates_table() {
        let db = make_test_db();
        let conn = db.conn();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_logs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_save_and_get_log() {
        let db = make_test_db();
        let log = make_log("r1", "claude-sonnet-4-6", "acc1", 1000);
        db.save_log(&log).unwrap();

        let logs = db.get_logs(10, 0, None, None, None, None, None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "r1");
        assert_eq!(logs[0].input_tokens, Some(1000));
        assert_eq!(logs[0].total_cost, Some(0.010935));
    }

    #[test]
    fn test_save_log_upsert_on_failover() {
        // Regression test for P2-B: failover to second account must correctly
        // overwrite the log record (account ownership + tokens) instead of
        // hitting a PRIMARY KEY conflict and silently leaving account A's data.
        let db = make_test_db();
        let log_id = "failover-req-1";

        // Step 1: Account A is tried first, save initial log (no tokens yet)
        let mut log_a = make_log(log_id, "claude-sonnet-4-6", "acc_a", 1000);
        log_a.input_tokens = Some(0);
        log_a.output_tokens = Some(0);
        log_a.total_cost = Some(0.0);
        db.save_log(&log_a).unwrap();

        // Step 2: Simulate partial usage being recorded against account A before failover
        // (in real code, this would happen via update_usage during SSE streaming)
        let conn = db.conn();
        conn.execute(
            "UPDATE request_logs SET input_tokens=?1, output_tokens=?2, total_cost=?3 WHERE id=?4",
            params![100i64, 50i64, 0.001f64, log_id],
        )
        .unwrap();
        drop(conn);

        // Step 3: Account A fails mid-stream, failover to account B.
        // Handler calls save_log again with the SAME log_id.
        let log_b = make_log(log_id, "claude-sonnet-4-6", "acc_b", 2000);
        db.save_log(&log_b).unwrap(); // Must not error — UPSERT path

        // Step 4: Verify account ownership switched to B and token counts were reset
        // (so subsequent update_usage calls from B's stream will accumulate correctly)
        let logs = db.get_logs(10, 0, None, None, None, None, None).unwrap();
        assert_eq!(logs.len(), 1, "should still be one record, not two");
        let row = &logs[0];
        assert_eq!(row.id, log_id);
        assert_eq!(row.account_id, Some("acc_b".to_string()), "ownership should switch to B");
        assert_eq!(row.timestamp, 2000, "timestamp should update to B's attempt");
        assert_eq!(row.input_tokens, Some(1000), "tokens reset to B's snapshot (1000)");
        assert_eq!(row.total_cost, Some(0.010935), "cost reset to B's snapshot");
    }

    #[test]
    fn test_get_logs_pagination() {
        let db = make_test_db();
        for i in 0..10 {
            let log = make_log(&format!("r{}", i), "claude-sonnet-4-6", "acc1", 1000 + i);
            db.save_log(&log).unwrap();
        }

        let page1 = db.get_logs(3, 0, None, None, None, None, None).unwrap();
        assert_eq!(page1.len(), 3);
        assert_eq!(page1[0].id, "r9");

        let page2 = db.get_logs(3, 3, None, None, None, None, None).unwrap();
        assert_eq!(page2.len(), 3);
        assert_eq!(page2[0].id, "r6");
    }

    #[test]
    fn test_get_logs_filter_by_account() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc2", 1001)).unwrap();

        let logs = db.get_logs(10, 0, Some("acc1"), None, None, None, None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].account_id, Some("acc1".to_string()));
    }

    #[test]
    fn test_get_logs_filter_by_model() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-opus-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc1", 1001)).unwrap();

        let logs = db.get_logs(10, 0, None, Some("claude-opus-4-6"), None, None, None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].model, Some("claude-opus-4-6".to_string()));
    }

    #[test]
    fn test_get_logs_filter_by_date_range() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc1", 2000)).unwrap();
        db.save_log(&make_log("r3", "claude-sonnet-4-6", "acc1", 3000)).unwrap();

        let logs = db.get_logs(10, 0, None, None, Some(1500), Some(2500), None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "r2");
    }

    #[test]
    fn test_token_stats_by_day() {
        let db = make_test_db();
        let ts1 = 1712188800000_i64;
        let ts2 = ts1 + 3600000;
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", ts1)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc1", ts2)).unwrap();

        let stats = db.get_token_stats("day", None).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].request_count, 2);
        assert_eq!(stats[0].total_input_tokens, 2000);
    }

    #[test]
    fn test_cost_summary() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-opus-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc2", 2000)).unwrap();

        let summary = db.get_cost_summary(0, 5000).unwrap();
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.by_model.len(), 2);
    }

    #[test]
    fn test_clear_logs_all() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc1", 2000)).unwrap();

        let deleted = db.clear_logs(None).unwrap();
        assert_eq!(deleted, 2);

        let logs = db.get_logs(10, 0, None, None, None, None, None).unwrap();
        assert!(logs.is_empty());
    }

    #[test]
    fn test_update_status_and_error_patches_existing_row() {
        let db = make_test_db();
        let log = make_log("r1", "claude-sonnet-4-6", "acc1", 1000);
        db.save_log(&log).unwrap();

        db.update_status_and_error("r1", 502, "sse first-byte timeout").unwrap();

        let logs = db.get_logs(10, 0, None, None, None, None, None).unwrap();
        assert_eq!(logs[0].status, 502);
        assert_eq!(logs[0].error.as_deref(), Some("sse first-byte timeout"));
        // Token fields untouched.
        assert_eq!(logs[0].input_tokens, Some(1000));
        assert_eq!(logs[0].output_tokens, Some(500));
    }

    #[test]
    fn test_update_status_and_error_missing_id_is_noop() {
        let db = make_test_db();
        // Updating a non-existent row must not error; UPDATE affects 0 rows.
        db.update_status_and_error("missing", 502, "foo").unwrap();
    }

    #[test]
    fn test_update_usage_only_modifies_token_fields() {
        let db = make_test_db();
        let mut log = make_log("r1", "claude-sonnet-4-6", "acc1", 1000);
        log.input_tokens = Some(100);
        log.output_tokens = Some(0);
        log.total_tokens = Some(100);
        log.input_cost = Some(0.0003);
        log.output_cost = Some(0.0);
        log.total_cost = Some(0.0003);
        db.save_log(&log).unwrap();

        let usage = crate::modules::billing::TokenUsage {
            input_tokens: 5000,
            output_tokens: 2000,
            cache_creation_tokens: 500,
            cache_read_tokens: 1000,
        };
        let cost = crate::modules::billing::calculate_cost("claude-sonnet-4-6", &usage);
        db.update_usage("r1", &usage, &cost).unwrap();

        let logs = db.get_logs(10, 0, None, None, None, None, None).unwrap();
        assert_eq!(logs[0].input_tokens, Some(5000));
        assert_eq!(logs[0].output_tokens, Some(2000));
        assert_eq!(logs[0].cache_creation_tokens, Some(500));
        assert_eq!(logs[0].cache_read_tokens, Some(1000));
        assert_eq!(logs[0].total_tokens, Some(8500));

        assert_eq!(logs[0].status, 200);
        assert_eq!(logs[0].model, Some("claude-sonnet-4-6".to_string()));
        assert_eq!(logs[0].account_id, Some("acc1".to_string()));
        assert_eq!(logs[0].duration_ms, 1500);
    }

    #[test]
    fn test_get_logs_count_no_filter() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-opus-4-6", "acc2", 2000)).unwrap();

        let count = db.get_logs_count(None).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_get_logs_count_with_search() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-opus-4-6", "acc2", 2000)).unwrap();

        let count = db.get_logs_count(Some("opus")).unwrap();
        assert_eq!(count, 1);

        let count = db.get_logs_count(Some("acc1")).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_logs_with_search_filter() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-opus-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc2", 2000)).unwrap();

        let logs = db.get_logs(10, 0, None, None, None, None, Some("opus")).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].model, Some("claude-opus-4-6".to_string()));

        let logs = db.get_logs(10, 0, None, None, None, None, Some("acc2")).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].account_email, Some("acc2@test.com".to_string()));

        let logs = db.get_logs(10, 0, None, None, None, None, Some("")).unwrap();
        assert_eq!(logs.len(), 2);
    }

    #[test]
    fn test_clear_logs_before_timestamp() {
        let db = make_test_db();
        db.save_log(&make_log("r1", "claude-sonnet-4-6", "acc1", 1000)).unwrap();
        db.save_log(&make_log("r2", "claude-sonnet-4-6", "acc1", 2000)).unwrap();
        db.save_log(&make_log("r3", "claude-sonnet-4-6", "acc1", 3000)).unwrap();

        let deleted = db.clear_logs(Some(2500)).unwrap();
        assert_eq!(deleted, 2);

        let logs = db.get_logs(10, 0, None, None, None, None, None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "r3");
    }
}
