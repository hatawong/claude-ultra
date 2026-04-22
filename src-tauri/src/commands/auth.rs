use serde::{Deserialize, Serialize};
use tauri::State;

use crate::modules::server_client::{AuthState, AuthUser};
#[cfg(not(feature = "internal"))]
use crate::modules::server_client::ServerClient;
use crate::GatewayServiceState;

// GitHub OAuth App client_id (public value, not a secret)
const GITHUB_CLIENT_ID: &str = "Ov23liv1cNHLrsULmJpa";

// ── Data types ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub mode: String,
    pub status: String,
    pub user: Option<AuthUserInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUserInfo {
    pub id: String,
    pub display_name: String,
    pub plan: String,
    pub max_accounts: u32,
    pub plan_expires_at: Option<u64>,
}

impl From<AuthUser> for AuthUserInfo {
    fn from(u: AuthUser) -> Self {
        Self {
            id: u.id,
            display_name: u.display_name,
            plan: u.plan,
            max_accounts: u.max_accounts,
            plan_expires_at: u.plan_expires_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFlowStart {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFlowResult {
    pub user: AuthUserInfo,
}

// ── GitHub Device Flow response types ───────────────────────────────

#[derive(Debug, Deserialize)]
struct GithubDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: Option<u64>,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

// ── Server auth response ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerAuthData {
    token: String,
    license_token: Option<String>,
    user: AuthUser,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivateData {
    plan: String,
    license_token: Option<String>,
    max_accounts: u32,
    plan_expires_at: Option<u64>,
}

// ── Commands ────────────────────────────────────────────────────────

/// Get current auth status
#[tauri::command]
pub async fn get_auth_status(
    state: State<'_, GatewayServiceState>,
) -> Result<AuthStatus, String> {
    // Internal mode: always "logged in", no user info needed
    #[cfg(feature = "internal")]
    {
        let _ = &state; // suppress unused warning
        return Ok(AuthStatus {
            mode: "internal".to_string(),
            status: "logged_in".to_string(),
            user: None,
        });
    }

    #[cfg(not(feature = "internal"))]
    {
        let server_client = &state.server_client;
        match server_client.get_auth_state() {
            Some(auth) => {
                // Check if JWT is expired
                if ServerClient::is_token_expired(&auth.token) {
                    server_client.clear_auth_state();
                    Ok(AuthStatus {
                        mode: "distribution".to_string(),
                        status: "not_logged_in".to_string(),
                        user: None,
                    })
                } else {
                    Ok(AuthStatus {
                        mode: "distribution".to_string(),
                        status: "logged_in".to_string(),
                        user: Some(auth.user.into()),
                    })
                }
            }
            None => Ok(AuthStatus {
                mode: "distribution".to_string(),
                status: "not_logged_in".to_string(),
                user: None,
            }),
        }
    }
}

/// Step 1: Start GitHub Device Flow — returns user_code + verification_uri
#[tauri::command]
pub async fn start_device_flow() -> Result<DeviceFlowStart, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("scope", "read:user"),
        ])
        .send()
        .await
        .map_err(|e| format!("Failed to start device flow: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub returned {}: {}", status, body));
    }

    let data: GithubDeviceCodeResponse = resp.json().await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))?;

    Ok(DeviceFlowStart {
        user_code: data.user_code,
        verification_uri: data.verification_uri,
        device_code: data.device_code,
        interval: data.interval.unwrap_or(5),
        expires_in: data.expires_in.unwrap_or(900),
    })
}

/// Step 2: Poll GitHub for access_token, then exchange with our Server for JWT
#[tauri::command]
pub async fn poll_device_flow(
    state: State<'_, GatewayServiceState>,
    device_code: String,
    interval: u64,
    expires_in: u64,
) -> Result<DeviceFlowResult, String> {
    let client = reqwest::Client::new();
    let mut poll_interval = std::time::Duration::from_secs(interval);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);

    loop {
        if std::time::Instant::now() >= deadline {
            return Err("Device flow timed out".to_string());
        }

        tokio::time::sleep(poll_interval).await;

        let resp = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", GITHUB_CLIENT_ID),
                ("device_code", device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|e| format!("Poll request failed: {}", e))?;

        let token_resp: GithubTokenResponse = resp.json().await
            .map_err(|e| format!("Failed to parse poll response: {}", e))?;

        // Check for access_token first
        if let Some(access_token) = token_resp.access_token {
            // Step 4: Exchange GitHub token for our Server JWT
            let server_client = &state.server_client;
            let auth_data: ServerAuthData = server_client
                .post_unauthenticated(
                    "/auth/github/device",
                    &serde_json::json!({ "github_token": access_token }),
                )
                .await?;

            // Save to memory + disk
            server_client.set_auth_state(AuthState {
                token: auth_data.token,
                license_token: auth_data.license_token,
                user: auth_data.user.clone(),
            })?;

            return Ok(DeviceFlowResult {
                user: auth_data.user.into(),
            });
        }

        // Handle error states
        match token_resp.error.as_deref() {
            Some("authorization_pending") => {
                // Continue polling with current interval
            }
            Some("slow_down") => {
                // Increase interval by 5 seconds
                let new_secs = poll_interval.as_secs() + 5;
                poll_interval = std::time::Duration::from_secs(new_secs);
                if let Some(new_interval) = token_resp.interval {
                    poll_interval = std::time::Duration::from_secs(new_interval);
                }
            }
            Some("expired_token") => {
                return Err("Authorization expired. Please start again.".to_string());
            }
            Some("access_denied") => {
                return Err("Authorization was denied by user.".to_string());
            }
            Some(other) => {
                return Err(format!("GitHub OAuth error: {}", other));
            }
            None => {
                return Err("Unexpected response from GitHub (no token, no error)".to_string());
            }
        }
    }
}

/// Logout: shutdown services → clear auth → frontend switches to Login
#[tauri::command]
pub async fn logout(
    app: tauri::AppHandle,
    state: State<'_, GatewayServiceState>,
) -> Result<(), String> {
    // Shutdown services first
    crate::shutdown_services_inner(&app).await?;

    // Clear auth state (memory + disk)
    state.server_client.clear_auth_state();

    tracing::info!("User logged out");
    Ok(())
}

/// Activate subscription with activation code
#[tauri::command]
pub async fn activate_subscription(
    state: State<'_, GatewayServiceState>,
    code: String,
) -> Result<AuthUserInfo, String> {
    let server_client = &state.server_client;

    let data: ActivateData = server_client
        .post("/subscription/activate", &serde_json::json!({ "code": code }))
        .await?;

    // Update in-memory + disk auth state with new plan info + license
    if let Some(mut auth) = server_client.get_auth_state() {
        auth.user.plan = data.plan;
        auth.user.max_accounts = data.max_accounts;
        auth.user.plan_expires_at = data.plan_expires_at;
        if let Some(ref lt) = data.license_token {
            // Verify and install new license before persisting
            claude_ultra_http::license::set_license(lt)
                .map_err(|e| format!("License update failed: {}. Please re-login.", e))?;
            auth.license_token = Some(lt.clone());
        }
        server_client.set_auth_state(auth.clone())?;
        Ok(auth.user.into())
    } else {
        Err("Not logged in".to_string())
    }
}

/// Initialize services (second-stage startup)
#[tauri::command]
pub async fn init_services(app: tauri::AppHandle) -> Result<(), String> {
    crate::init_services_inner(&app).await
}

/// Shutdown services (for logout)
#[tauri::command]
pub async fn shutdown_services(app: tauri::AppHandle) -> Result<(), String> {
    crate::shutdown_services_inner(&app).await
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_status_serialization() {
        let status = AuthStatus {
            mode: "distribution".to_string(),
            status: "logged_in".to_string(),
            user: Some(AuthUserInfo {
                id: "u1".to_string(),
                display_name: "testuser".to_string(),
                plan: "pro".to_string(),
                max_accounts: 3,
                plan_expires_at: Some(1720000000000),
            }),
        };

        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["mode"], "distribution");
        assert_eq!(json["status"], "logged_in");
        assert_eq!(json["user"]["displayName"], "testuser");
        assert_eq!(json["user"]["plan"], "pro");
        assert_eq!(json["user"]["maxAccounts"], 3);
        assert_eq!(json["user"]["planExpiresAt"], 1720000000000u64);
    }

    #[test]
    fn test_auth_status_not_logged_in() {
        let status = AuthStatus {
            mode: "distribution".to_string(),
            status: "not_logged_in".to_string(),
            user: None,
        };

        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], "not_logged_in");
        assert!(json["user"].is_null());
    }

    #[test]
    fn test_auth_status_internal_mode() {
        let status = AuthStatus {
            mode: "internal".to_string(),
            status: "logged_in".to_string(),
            user: None,
        };

        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["mode"], "internal");
        assert_eq!(json["status"], "logged_in");
    }

    #[test]
    fn test_device_flow_start_serialization() {
        let start = DeviceFlowStart {
            user_code: "ABCD-1234".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            device_code: "dc_xxx".to_string(),
            interval: 5,
            expires_in: 900,
        };

        let json = serde_json::to_value(&start).unwrap();
        assert_eq!(json["userCode"], "ABCD-1234");
        assert_eq!(json["verificationUri"], "https://github.com/login/device");
        assert_eq!(json["deviceCode"], "dc_xxx");
        assert_eq!(json["interval"], 5);
        assert_eq!(json["expiresIn"], 900);
    }

    #[test]
    fn test_auth_user_info_from_auth_user() {
        let user = AuthUser {
            id: "u1".to_string(),
            display_name: "hata".to_string(),
            plan: "max".to_string(),
            max_accounts: 10,
            plan_expires_at: Some(1720000000000),
        };

        let info: AuthUserInfo = user.into();
        assert_eq!(info.id, "u1");
        assert_eq!(info.display_name, "hata");
        assert_eq!(info.plan, "max");
        assert_eq!(info.max_accounts, 10);
        assert_eq!(info.plan_expires_at, Some(1720000000000));
    }
}
