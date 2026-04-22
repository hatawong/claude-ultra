use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

/// User info from Server auth response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub id: String,
    pub display_name: String,
    pub plan: String,
    pub max_accounts: u32,
    pub plan_expires_at: Option<u64>,
}

/// Persisted auth state (auth.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthFile {
    pub version: u32,
    pub token: String,
    #[serde(default)]
    pub license_token: Option<String>,
    pub user: AuthUser,
}

/// In-memory auth state
#[derive(Debug, Clone)]
pub struct AuthState {
    pub token: String,
    pub license_token: Option<String>,
    pub user: AuthUser,
}

/// Server API client — reqwest direct (no BoringClient/proxy)
pub struct ServerClient {
    pub base_url: String,
    http: reqwest::Client,
    auth: RwLock<Option<AuthState>>,
}

/// Typed wrapper for Server API responses
#[derive(Debug, Deserialize)]
pub struct ServerResponse<T> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<ServerError>,
}

#[derive(Debug, Deserialize)]
pub struct ServerError {
    pub code: String,
    pub message: String,
}

impl ServerClient {
    pub fn new(base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client");

        let auth = Self::load_auth_file()
            .and_then(|f| {
                // Check JWT expiry before trusting persisted state
                if Self::is_token_expired(&f.token) {
                    tracing::info!("Persisted JWT expired, clearing auth state");
                    None
                } else {
                    Some(AuthState {
                        token: f.token,
                        license_token: f.license_token,
                        user: f.user,
                    })
                }
            });

        Self {
            base_url,
            http,
            auth: RwLock::new(auth),
        }
    }

    /// Get current auth state (from memory)
    pub fn get_auth_state(&self) -> Option<AuthState> {
        self.auth.read().unwrap().clone()
    }

    /// Set auth state (memory + disk)
    pub fn set_auth_state(&self, state: AuthState) -> Result<(), String> {
        let file = AuthFile {
            version: 1,
            token: state.token.clone(),
            license_token: state.license_token.clone(),
            user: state.user.clone(),
        };
        Self::save_auth_file(&file)?;
        *self.auth.write().unwrap() = Some(state);
        Ok(())
    }

    /// Clear auth state (memory + disk)
    pub fn clear_auth_state(&self) {
        *self.auth.write().unwrap() = None;
        let _ = Self::delete_auth_file();
    }

    /// Make authenticated GET request
    pub async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.get(&url);

        if let Some(auth) = self.get_auth_state() {
            req = req.header("Authorization", format!("Bearer {}", auth.token));
        }

        let resp = req.send().await
            .map_err(|e| format!("Request failed: {}", e))?;

        let status = resp.status();
        let body: ServerResponse<T> = resp.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        if body.ok {
            body.data.ok_or_else(|| "Response ok but no data".to_string())
        } else {
            let err = body.error.unwrap_or(ServerError {
                code: status.as_str().to_string(),
                message: "Unknown error".to_string(),
            });
            Err(format!("{}: {}", err.code, err.message))
        }
    }

    /// Make authenticated POST request with JSON body
    pub async fn post<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(body);

        if let Some(auth) = self.get_auth_state() {
            req = req.header("Authorization", format!("Bearer {}", auth.token));
        }

        let resp = req.send().await
            .map_err(|e| format!("Request failed: {}", e))?;

        let status = resp.status();
        let body: ServerResponse<T> = resp.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        if body.ok {
            body.data.ok_or_else(|| "Response ok but no data".to_string())
        } else {
            let err = body.error.unwrap_or(ServerError {
                code: status.as_str().to_string(),
                message: "Unknown error".to_string(),
            });
            Err(format!("{}: {}", err.code, err.message))
        }
    }

    /// Make unauthenticated POST request with JSON body (for device flow token exchange)
    pub async fn post_unauthenticated<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.post(&url).json(body).send().await
            .map_err(|e| format!("Request failed: {}", e))?;

        let status = resp.status();
        let body: ServerResponse<T> = resp.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        if body.ok {
            body.data.ok_or_else(|| "Response ok but no data".to_string())
        } else {
            let err = body.error.unwrap_or(ServerError {
                code: status.as_str().to_string(),
                message: "Unknown error".to_string(),
            });
            Err(format!("{}: {}", err.code, err.message))
        }
    }

    // ── auth.json persistence ───────────────────────────────────────

    fn auth_file_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude-ultra").join("auth.json"))
    }

    fn load_auth_file() -> Option<AuthFile> {
        let path = Self::auth_file_path()?;
        let content = fs::read_to_string(&path).ok()?;
        let file: AuthFile = serde_json::from_str(&content).ok()?;
        // Version check — only v1 supported
        if file.version != 1 {
            tracing::warn!("Unsupported auth.json version: {}", file.version);
            return None;
        }
        Some(file)
    }

    fn save_auth_file(file: &AuthFile) -> Result<(), String> {
        let path = Self::auth_file_path()
            .ok_or("Cannot determine auth file path")?;
        // Atomic write + 0600: auth.json contains GitHub OAuth access_token + refresh_token
        crate::modules::secure_fs::secure_write_json(&path, file)
    }

    fn delete_auth_file() -> Result<(), String> {
        if let Some(path) = Self::auth_file_path() {
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|e| format!("Failed to delete auth.json: {}", e))?;
            }
        }
        Ok(())
    }

    /// Check if a JWT token is expired.
    /// JWT payload is base64url-encoded JSON with `exp` field (seconds since epoch).
    pub fn is_token_expired(token: &str) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return true;
        }
        // Decode payload (base64url → JSON)
        let payload = match base64_url_decode(parts[1]) {
            Some(p) => p,
            None => return true,
        };
        let json: serde_json::Value = match serde_json::from_slice(&payload) {
            Ok(v) => v,
            Err(_) => return true,
        };
        let exp = match json.get("exp").and_then(|v| v.as_u64()) {
            Some(e) => e,
            None => return true,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now >= exp
    }
}

/// Base64url decode (no padding)
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    engine.decode(input).ok()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");

        let file = AuthFile {
            version: 1,
            token: "test-jwt-token".to_string(),
            license_token: None,
            user: AuthUser {
                id: "user-123".to_string(),
                display_name: "testuser".to_string(),
                plan: "free".to_string(),
                max_accounts: 1,
                plan_expires_at: None,
            },
        };

        let content = serde_json::to_string_pretty(&file).unwrap();
        fs::write(&path, &content).unwrap();

        let loaded: AuthFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.token, "test-jwt-token");
        assert_eq!(loaded.user.display_name, "testuser");
        assert_eq!(loaded.user.plan, "free");
        assert_eq!(loaded.user.max_accounts, 1);
        assert!(loaded.user.plan_expires_at.is_none());
    }

    #[test]
    fn test_auth_file_with_plan_expiry() {
        let file = AuthFile {
            version: 1,
            token: "jwt".to_string(),
            license_token: Some("license-jwt".to_string()),
            user: AuthUser {
                id: "u1".to_string(),
                display_name: "pro_user".to_string(),
                plan: "pro".to_string(),
                max_accounts: 3,
                plan_expires_at: Some(1720000000000), // milliseconds
            },
        };

        let json = serde_json::to_string(&file).unwrap();
        let restored: AuthFile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.user.plan_expires_at, Some(1720000000000));
        assert_eq!(restored.user.max_accounts, 3);
    }

    #[test]
    fn test_jwt_expiry_check() {
        // Expired token (exp = 0)
        // Header: {"alg":"HS256","typ":"JWT"}, Payload: {"exp":0}, Signature: dummy
        let expired = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjB9.dummy";
        assert!(ServerClient::is_token_expired(expired));

        // Far-future token (exp = 9999999999, year ~2286)
        let valid = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.dummy";
        assert!(!ServerClient::is_token_expired(valid));

        // Invalid token
        assert!(ServerClient::is_token_expired("not-a-jwt"));
        assert!(ServerClient::is_token_expired(""));
    }

    #[test]
    fn test_unsupported_version() {
        let json = r#"{"version":99,"token":"x","user":{"id":"u","displayName":"n","plan":"free","maxAccounts":1,"planExpiresAt":null}}"#;
        let file: AuthFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.version, 99);
        // load_auth_file would reject version != 1
    }

    #[test]
    fn test_server_client_memory_state() {
        let client = ServerClient::new("http://localhost:8787".to_string());
        // Clear memory state only (don't touch disk — real auth.json may exist from smoke tests)
        *client.auth.write().unwrap() = None;

        let state = AuthState {
            token: "test-token".to_string(),
            license_token: None,
            user: AuthUser {
                id: "u1".to_string(),
                display_name: "test".to_string(),
                plan: "free".to_string(),
                max_accounts: 1,
                plan_expires_at: None,
            },
        };

        // set_auth_state will try to write to disk — may fail in test env, test memory only
        *client.auth.write().unwrap() = Some(state.clone());
        let loaded = client.get_auth_state().unwrap();
        assert_eq!(loaded.token, "test-token");
        assert_eq!(loaded.user.display_name, "test");

        // clear
        *client.auth.write().unwrap() = None;
        assert!(client.get_auth_state().is_none());
    }

    #[test]
    fn test_base_url_formatting() {
        let client = ServerClient::new("http://localhost:8787".to_string());
        assert_eq!(client.base_url, "http://localhost:8787");
    }
}
