//! Static proxy IPC commands — UI test connection + write ProxySection snapshot.

use crate::commands::proxy::ProxyTestResult;
use crate::models::account::{ProxySection, StaticProxy};

/// Strip credentials (raw + percent-encoded) from a log/error string. reqwest
/// URL parse errors and underlying socket errors may contain the full
/// `user:pass@host:port` authority; logging that verbatim leaks the static
/// proxy credentials to disk where users sharing logs would expose them.
fn redact_credentials(s: &str, sp: &StaticProxy) -> String {
    let mut out = s.to_string();
    let needles = [&sp.username, &sp.password];
    for n in needles.iter().filter(|s| !s.is_empty()) {
        out = out.replace(n.as_str(), "<REDACTED>");
        // Common percent-encoding (matches build_static_proxy_url percent_encode_alnum).
        let encoded: String = n.bytes().map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{:02X}", b)
            }
        }).collect();
        if encoded != **n {
            out = out.replace(&encoded, "<REDACTED>");
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct IpApiResponse {
    query: Option<String>,
    country: Option<String>,
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    city: Option<String>,
    isp: Option<String>,
    status: Option<String>,
    #[serde(default)]
    mobile: bool,
    #[serde(default)]
    proxy: bool,
    #[serde(default)]
    hosting: bool,
}

/// Run a one-shot static-proxy reachability test against ip-api.com.
/// Returns a ProxyTestResult including a ProxySection snapshot on success.
/// Pure function — no account_id, no disk writes. Both the dryrun IPC
/// (no accountId) and the connection IPC (with accountId + same-creds
/// refresh persistence) call this. Persistence is layered on top by the
/// connection IPC, not here.
async fn run_static_proxy_test_inner(static_proxy: &StaticProxy) -> ProxyTestResult {
    let url = crate::proxy::allocator::build_static_proxy_url(static_proxy);
    // reqwest 0.12 socks5 (client-side DNS) is incompatible with some SOCKS5
    // servers (observed: IPFoxy returns "connection refused" through reqwest
    // but works via curl with socks5h://). Use socks5h:// (server-side DNS)
    // for the test path to mirror curl behavior. http:// (HTTP CONNECT) path
    // unaffected. Production transport (handler::ActualRoute::Static) uses
    // BoringClient's tokio-socks integration which does not have this issue.
    let test_url = if static_proxy.protocol == "socks5" {
        url.replacen("socks5://", "socks5h://", 1)
    } else {
        url.clone()
    };
    tracing::info!(
        "[STATIC_PROXY] test start: protocol={} host={} port={} user_len={}",
        static_proxy.protocol, static_proxy.host, static_proxy.port,
        static_proxy.username.len(),
    );
    let proxy = match reqwest::Proxy::all(&test_url) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "[STATIC_PROXY] Invalid static proxy URL (host={} port={}): {}",
                static_proxy.host, static_proxy.port,
                redact_credentials(&e.to_string(), static_proxy),
            );
            return ProxyTestResult {
                ok: false, mode: "proxied".into(),
                ip: None, country: None,
                error: Some("Invalid static proxy URL".into()),
                proxy_section: None,
            };
        }
    };
    let client = match reqwest::Client::builder()
        .proxy(proxy)
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[STATIC_PROXY] Failed to create proxy client: {}", e);
            return ProxyTestResult {
                ok: false, mode: "proxied".into(),
                ip: None, country: None,
                error: Some("Failed to create proxy client".into()),
                proxy_section: None,
            };
        }
    };

    // Request extra fields so the ProxySection snapshot has ISP/region/city.
    let endpoint = "http://ip-api.com/json/?fields=status,query,country,regionName,city,isp,mobile,proxy,hosting";
    match client.get(endpoint).send().await {
        Ok(resp) => {
            tracing::info!("[STATIC_PROXY] test send ok, status={}", resp.status());
            // Triage by HTTP status BEFORE attempting JSON parse — a 407
            // (proxy auth fail) or 5xx (upstream IP dead) returns an empty
            // body that masquerades as "parse failure" otherwise, hiding
            // the real cause from the user.
            let status = resp.status();
            if status.as_u16() == 407 {
                return ProxyTestResult {
                    ok: false, mode: "proxied".into(),
                    ip: None, country: None,
                    error: Some(
                        "Static proxy authentication failed (HTTP 407). Check username and password.".into(),
                    ),
                    proxy_section: None,
                };
            }
            if status.as_u16() == 502 || status.as_u16() == 503 || status.as_u16() == 504 {
                return ProxyTestResult {
                    ok: false, mode: "proxied".into(),
                    ip: None, country: None,
                    error: Some(format!(
                        "Static proxy upstream unavailable (HTTP {}). The static IP did not respond — try again or rotate to a different upstream.",
                        status.as_u16()
                    )),
                    proxy_section: None,
                };
            }
            if !status.is_success() {
                return ProxyTestResult {
                    ok: false, mode: "proxied".into(),
                    ip: None, country: None,
                    error: Some(format!(
                        "Static proxy returned HTTP {} from ip-api.com lookup.",
                        status.as_u16()
                    )),
                    proxy_section: None,
                };
            }
            let body_bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => return ProxyTestResult {
                    ok: false, mode: "proxied".into(),
                    ip: None, country: None,
                    error: Some(format!("Failed to read response body: {}", e)),
                    proxy_section: None,
                },
            };
            let data: IpApiResponse = match serde_json::from_slice(&body_bytes) {
                Ok(d) => d,
                Err(_) => {
                    let snippet = String::from_utf8_lossy(&body_bytes);
                    let snippet = snippet.chars().take(120).collect::<String>();
                    return ProxyTestResult {
                        ok: false, mode: "proxied".into(),
                        ip: None, country: None,
                        error: Some(format!(
                            "Static proxy returned non-JSON body (likely an upstream captive portal): {}",
                            snippet.trim()
                        )),
                        proxy_section: None,
                    };
                }
            };
            if data.status.as_deref() != Some("success") {
                return ProxyTestResult {
                    ok: false, mode: "proxied".into(),
                    ip: None, country: None,
                    error: Some("IP lookup failed (ip-api.com reported non-success status).".into()),
                    proxy_section: None,
                };
            }
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            // Quality classification mirrors webapp fetchIpInfo:
            // mobile > datacenter (hosting|proxy) > residential.
            let quality = if data.mobile { "mobile" }
                else if data.hosting || data.proxy { "datacenter" }
                else { "residential" };
            // Stable session_id derived from host:port (static IP is 1:1
            // with host:port).
            let snapshot = ProxySection {
                session_id: format!("static-{}-{}", static_proxy.host, static_proxy.port),
                host: format!("{}:{}", static_proxy.host, static_proxy.port),
                last_ip: data.query.clone(),
                country: data.country.clone(),
                region: data.region_name.clone(),
                city: data.city.clone(),
                isp: data.isp.clone(),
                quality: Some(quality.to_string()),
                last_checked: Some(now_ms),
                proxy_type: Some("residential".to_string()),
                lifetime: Some("static".to_string()),
                created_at: Some(now_ms),
            };
            tracing::info!(
                "[STATIC_PROXY] test ok: ip={:?} country={:?} isp={:?}",
                data.query, data.country, data.isp,
            );
            ProxyTestResult {
                ok: true, mode: "proxied".into(),
                ip: data.query, country: data.country,
                error: None,
                proxy_section: Some(snapshot),
            }
        }
        Err(send_err) => {
            // Walk source chain to tracing log so operators can diagnose root
            // cause (ECONNREFUSED / SOCKS5 protocol / auth / DNS). UI message
            // stays user-friendly aligned with the dynamic test_proxy_connection.
            let mut chain = Vec::<String>::new();
            let mut current: Option<&dyn std::error::Error> = Some(&send_err);
            while let Some(e) = current {
                chain.push(e.to_string());
                current = e.source();
            }
            // Redact credentials before logging — error chain (reqwest URL
            // parse / socket errors) may contain full user:pass@host:port.
            tracing::warn!(
                "[STATIC_PROXY] Static proxy connection failed (host={} port={}): {}",
                static_proxy.host, static_proxy.port,
                redact_credentials(&chain.join(" -> "), static_proxy),
            );
            ProxyTestResult {
                ok: false, mode: "proxied".into(),
                ip: None, country: None,
                error: Some(
                    "Static proxy connection failed. Check host, port, and credentials.".into(),
                ),
                proxy_section: None,
            }
        }
    }
}

/// Test a static proxy connection. On success, write a ProxySection snapshot
/// to the account when the incoming creds match the disk creds (same-creds
/// refresh) so AccountTable / RouteTooltip pick up the new IP/country/ISP.
/// Static proxies are intentionally not part of ProxyPool, so the snapshot
/// is one-shot at test time, not continuous monitoring.
#[tauri::command]
pub async fn test_static_proxy_connection(
    account_id: String,
    static_proxy: StaticProxy,
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<ProxyTestResult, String> {
    let result = run_static_proxy_test_inner(&static_proxy).await;
    if !result.ok {
        return Ok(result);
    }
    // Same-creds refresh: incoming staticProxy matches disk → user is
    // refreshing the snapshot for existing creds; persist directly.
    // Otherwise leave a.proxy untouched — the snapshot still travels back
    // inside ProxyTestResult so the Save path can forward it via
    // update_account_route.probed_proxy for atomic write of route_mode +
    // staticProxy + proxy.
    let on_disk = state.account_manager
        .read(&account_id).await.ok()
        .and_then(|a| a.static_proxy);
    if on_disk.as_ref() == Some(&static_proxy) {
        if let Some(snapshot) = result.proxy_section.clone() {
            if let Err(e) = state.account_manager
                .set_proxy(&account_id, Some(snapshot)).await
            {
                tracing::warn!(
                    "[STATIC_PROXY] set_proxy failed (snapshot not written): {}", e,
                );
            } else {
                tracing::info!(
                    "[STATIC_PROXY] snapshot persisted (same-creds refresh)",
                );
            }
        }
    } else {
        tracing::info!(
            "[STATIC_PROXY] snapshot deferred (creds differ from disk; await Save)",
        );
    }
    Ok(result)
}

/// Test a static proxy without writing to disk. Used by AddAccountDialog
/// where there is no accountId yet — the UI captures the resulting
/// ProxySection snapshot in parent state and forwards it to
/// add_account_and_login as `probed_proxy` for atomic write at submit time.
#[tauri::command]
pub async fn test_static_proxy_dryrun(
    static_proxy: StaticProxy,
) -> Result<ProxyTestResult, String> {
    Ok(run_static_proxy_test_inner(&static_proxy).await)
}
