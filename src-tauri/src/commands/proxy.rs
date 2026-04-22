use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub ok: bool,
    pub mode: String,
    pub ip: Option<String>,
    pub country: Option<String>,
    pub error: Option<String>,
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
        let r = &config.proxy.residential;
        let country = r.default_country.trim().to_lowercase();
        let password_with_country = if country.is_empty() {
            r.password.clone()
        } else {
            format!("{}_country-{}", r.password, country)
        };
        let proxy = reqwest::Proxy::all(&format!("http://{}:{}", r.host, r.port))
            .map_err(|_| "Invalid proxy host/port".to_string())?
            .basic_auth(&r.username, &password_with_country);
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

    match client.get("http://ip-api.com/json").send().await {
        Ok(resp) => {
            if let Ok(data) = resp.json::<IpApiResponse>().await {
                if data.status.as_deref() == Some("success") {
                    Ok(ProxyTestResult {
                        ok: true,
                        mode: if is_configured { "proxied".into() } else { "direct".into() },
                        ip: data.query,
                        country: data.country,
                        error: None,
                    })
                } else {
                    Ok(ProxyTestResult {
                        ok: false,
                        mode: if is_configured { "proxied".into() } else { "direct".into() },
                        ip: None,
                        country: None,
                        error: Some("IP lookup failed".into()),
                    })
                }
            } else {
                Ok(ProxyTestResult {
                    ok: false,
                    mode: if is_configured { "proxied".into() } else { "direct".into() },
                    ip: None,
                    country: None,
                    error: Some("Failed to parse response".into()),
                })
            }
        }
        Err(_) => Ok(ProxyTestResult {
            ok: false,
            mode: if is_configured { "proxied".into() } else { "direct".into() },
            ip: None,
            country: None,
            error: Some(if is_configured {
                "Proxy connection failed. Check host, port, and credentials.".into()
            } else {
                "Direct connection failed. Check network.".into()
            }),
        }),
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
