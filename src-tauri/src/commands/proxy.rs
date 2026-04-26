use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub ok: bool,
    pub mode: String,
    pub ip: Option<String>,
    pub country: Option<String>,
    pub error: Option<String>,
    /// Static proxy probe path returns the full ProxySection snapshot so the
    /// UI can hand it back via update_account_route's probed_proxy field on
    /// Save, committing the test result atomically with route_mode/staticProxy.
    /// Always None for the dynamic test_proxy_connection path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_section: Option<crate::models::account::ProxySection>,
}

#[derive(serde::Deserialize)]
struct IpApiResponse {
    query: Option<String>,
    country: Option<String>,
    status: Option<String>,
}

#[tauri::command]
pub async fn test_proxy_connection() -> Result<ProxyTestResult, String> {
    let config = crate::models::config::load_app_config().unwrap_or_default();
    let is_configured = crate::models::config::is_proxy_configured(&config);

    let client = if is_configured {
        // Build the probe URL from the freshly-loaded config, NOT from
        // state.proxy_allocator: the allocator's provider_config is an
        // immutable Arc captured at startup, so editing credentials in
        // Settings does not propagate there until the next launch. The
        // probe must reflect what the user just saved, otherwise it
        // tests stale credentials and reports a misleading failure.
        // The URL builder itself stays shared with the allocator so the
        // probe and production paths cannot diverge on format.
        // sessid is freshly generated per probe — a fixed sessid sticks
        // to a single upstream IP, and a single bad IP masks the rest
        // of the pool's health.
        let country_raw = config.proxy.default_country.trim().to_lowercase();
        let country = if country_raw.is_empty() { "us".to_string() } else { country_raw };
        let session_id = crate::proxy::allocator::ProxyAllocator::generate_session_id();
        let provider_cfg = crate::proxy_provider_config_from(&config);
        let proxy_url = crate::proxy::allocator::build_residential_proxy_url(
            &provider_cfg,
            &country,
            &session_id,
        );
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(|_| "Invalid proxy URL (check host/port/credentials)".to_string())?;
        reqwest::Client::builder()
            .proxy(proxy)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| "Failed to create proxy client".to_string())?
    } else {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?
    };

    let mode_label = if is_configured { "proxied" } else { "direct" };
    let fail = |error: String| ProxyTestResult {
        ok: false,
        mode: mode_label.into(),
        ip: None,
        country: None,
        error: Some(error),
        proxy_section: None,
    };

    match client.get("http://ip-api.com/json").send().await {
        Ok(resp) => {
            // Triage by HTTP status BEFORE attempting JSON parse: a 407 from
            // the proxy returns an empty body that masquerades as "parse
            // failure" otherwise, hiding the real cause from the user.
            let status = resp.status();
            if status.as_u16() == 407 {
                return Ok(fail(
                    "Proxy authentication failed (HTTP 407). Check username and password.".into(),
                ));
            }
            if status.as_u16() == 502 || status.as_u16() == 503 || status.as_u16() == 504 {
                return Ok(fail(format!(
                    "Proxy upstream unavailable (HTTP {}). The proxy connected but the residential exit returned no response — try again or change country/session.",
                    status.as_u16()
                )));
            }
            if !status.is_success() {
                return Ok(fail(format!(
                    "Proxy returned HTTP {} from ip-api.com lookup. Check proxy provider status.",
                    status.as_u16()
                )));
            }
            let body_bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => return Ok(fail(format!("Failed to read response body: {}", e))),
            };
            match serde_json::from_slice::<IpApiResponse>(&body_bytes) {
                Ok(data) => {
                    if data.status.as_deref() == Some("success") {
                        Ok(ProxyTestResult {
                            ok: true,
                            mode: mode_label.into(),
                            ip: data.query,
                            country: data.country,
                            error: None,
                            proxy_section: None,
                        })
                    } else {
                        Ok(fail("IP lookup failed (ip-api.com reported non-success status).".into()))
                    }
                }
                Err(_) => {
                    // Body was 200 but not JSON — likely a captive portal /
                    // upstream HTML error page tunnelled through the proxy.
                    // Surface a snippet so the user can act on the real cause.
                    let snippet = String::from_utf8_lossy(&body_bytes);
                    let snippet = snippet.chars().take(120).collect::<String>();
                    Ok(fail(format!(
                        "Proxy returned non-JSON body (likely an upstream captive portal): {}",
                        snippet.trim()
                    )))
                }
            }
        }
        Err(send_err) => {
            // Walk the error source chain so reqwest's wrapper (e.g.
            // "error sending request" → "client error (Connect)" → "tcp
            // connect error" → "Connection refused") surfaces to the user
            // rather than a generic catch-all.
            let mut chain = Vec::<String>::new();
            let mut current: Option<&dyn std::error::Error> = Some(&send_err);
            while let Some(e) = current {
                chain.push(e.to_string());
                current = e.source();
            }
            let detail = chain.join(" → ");
            Ok(fail(if is_configured {
                format!(
                    "Proxy connection failed: {}. Check host, port, and credentials.",
                    detail
                )
            } else {
                format!("Direct connection failed: {}. Check network.", detail)
            }))
        }
    }
}

/// Get current runtime proxy mode (from GatewayServiceState, not config file)
#[tauri::command]
pub async fn get_proxy_mode(
    state: tauri::State<'_, crate::GatewayServiceState>,
) -> Result<String, String> {
    Ok(match state.proxy_mode {
        crate::models::config::ProxyMode::Proxied => "proxied".to_string(),
        crate::models::config::ProxyMode::Direct => "direct".to_string(),
    })
}
