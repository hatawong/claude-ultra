#[derive(Debug, Clone, PartialEq)]
pub enum ActualRoute {
    Proxy(String),
    Static,
    Vercel,
    Direct,
}

impl ActualRoute {
    pub fn is_fallback(&self, intended_mode: &str) -> bool {
        !matches!(
            (intended_mode, self),
            ("proxy", ActualRoute::Proxy(_))
                | ("static", ActualRoute::Static)
                | ("vercel", ActualRoute::Vercel)
                | ("direct", ActualRoute::Direct)
        )
    }

    pub fn label(&self) -> String {
        match self {
            ActualRoute::Proxy(c) => format!("Proxy/{}", c.to_uppercase()),
            ActualRoute::Static => "Static".to_string(),
            ActualRoute::Vercel => "Vercel".to_string(),
            ActualRoute::Direct => "Direct".to_string(),
        }
    }
}

pub const PROXY_COUNTRIES: &[&str] = &["us", "jp", "kr", "ph"];

pub fn resolve_route(
    route_mode: &str,
    proxy_country: &str,
    has_proxy: bool,
    has_static: bool,
    has_vercel: bool,
) -> ActualRoute {
    let country = || proxy_country.to_lowercase();

    // Three-way symmetric fallback chain (Proxy / Static / Vercel).
    //
    // Each non-Direct mode tries its own transport first, then falls back to
    // the other two before exposing the user's real IP via Direct. Within the
    // fallback chain, paid/configured transports (Static, Vercel) take
    // priority over the dynamic IPRoyal pool (Proxy):
    //
    //   "proxy"  → has_proxy  → Static → Vercel → Direct
    //   "static" → has_static → Vercel → Proxy  → Direct
    //   "vercel" → has_vercel → Static → Proxy  → Direct
    //
    // Note on static safety: when staticProxy is missing, falling through to
    // Vercel/Proxy is preferred over Direct because no "established static IP
    // profile" exists yet to pollute. The UI is expected to prevent this state
    // (routeMode=static must require staticProxy), but the fallback chain is
    // a safety net for race windows / hand-edited JSON / older data.
    match route_mode {
        "proxy" => {
            if has_proxy {
                ActualRoute::Proxy(country())
            } else if has_static {
                ActualRoute::Static
            } else if has_vercel {
                ActualRoute::Vercel
            } else {
                ActualRoute::Direct
            }
        }
        "static" => {
            if has_static {
                ActualRoute::Static
            } else {
                // Hot path: this is called per-request. Demoted from warn to
                // debug so a misconfigured static account does not flood the
                // log. set_route is expected to surface the misconfiguration
                // once at the configuration entry point.
                tracing::debug!(
                    "route_mode='static' but staticProxy not configured; falling back through Vercel/Proxy chain"
                );
                if has_vercel {
                    ActualRoute::Vercel
                } else if has_proxy {
                    ActualRoute::Proxy(country())
                } else {
                    ActualRoute::Direct
                }
            }
        }
        "vercel" => {
            if has_vercel {
                ActualRoute::Vercel
            } else if has_static {
                ActualRoute::Static
            } else if has_proxy {
                ActualRoute::Proxy(country())
            } else {
                ActualRoute::Direct
            }
        }
        "direct" => ActualRoute::Direct,
        other => {
            tracing::warn!("unknown route_mode '{}', falling back to proxy chain", other);
            // Same fallback order as the "proxy" arm: Proxy → Static → Vercel → Direct.
            // Static must precede Direct so an account with configured staticProxy
            // never silently exits via the user's real IP under an unknown mode
            // (typo in hand-edited JSON, stale schema, etc.).
            if has_proxy {
                ActualRoute::Proxy(country())
            } else if has_static {
                ActualRoute::Static
            } else if has_vercel {
                ActualRoute::Vercel
            } else {
                ActualRoute::Direct
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(mode: &str, country: &str, hp: bool, hs: bool, hv: bool) -> ActualRoute {
        resolve_route(mode, country, hp, hs, hv)
    }

    #[test]
    fn proxy_normal() {
        assert_eq!(resolve("proxy", "jp", true, false, false), ActualRoute::Proxy("jp".into()));
    }

    #[test]
    fn proxy_fallback_vercel() {
        assert_eq!(resolve("proxy", "jp", false, false, true), ActualRoute::Vercel);
    }

    #[test]
    fn proxy_fallback_direct() {
        assert_eq!(resolve("proxy", "jp", false, false, false), ActualRoute::Direct);
    }

    #[test]
    fn vercel_normal() {
        assert_eq!(resolve("vercel", "us", false, false, true), ActualRoute::Vercel);
    }

    #[test]
    fn vercel_fallback_proxy() {
        assert_eq!(resolve("vercel", "us", true, false, false), ActualRoute::Proxy("us".into()));
    }

    #[test]
    fn vercel_fallback_direct() {
        assert_eq!(resolve("vercel", "us", false, false, false), ActualRoute::Direct);
    }

    #[test]
    fn direct_always() {
        assert_eq!(resolve("direct", "jp", true, false, true), ActualRoute::Direct);
    }

    #[test]
    fn static_with_creds_returns_static() {
        assert_eq!(resolve("static", "us", false, true, false), ActualRoute::Static);
    }

    #[test]
    fn static_no_creds_fallback_chain() {
        // static without creds falls through Vercel → Proxy → Direct (paid/
        // configured transports preferred over Direct's real-IP exposure).
        assert_eq!(resolve("static", "us", false, false, true), ActualRoute::Vercel);
        assert_eq!(resolve("static", "us", true,  false, false), ActualRoute::Proxy("us".into()));
        assert_eq!(resolve("static", "us", false, false, false), ActualRoute::Direct);
        // Vercel preferred over Proxy when both available.
        assert_eq!(resolve("static", "us", true,  false, true),  ActualRoute::Vercel);
    }

    #[test]
    fn proxy_fallback_chain_includes_static() {
        // proxy without creds: prefer Static (paid) over Vercel.
        assert_eq!(resolve("proxy", "jp", false, true, false), ActualRoute::Static);
        // Static preferred over Vercel when both available.
        assert_eq!(resolve("proxy", "jp", false, true, true),  ActualRoute::Static);
    }

    #[test]
    fn vercel_fallback_chain_includes_static() {
        // vercel without creds: prefer Static over Proxy (paid > dynamic pool).
        assert_eq!(resolve("vercel", "us", false, true, false), ActualRoute::Static);
        assert_eq!(resolve("vercel", "us", true,  true, false), ActualRoute::Static);
        assert_eq!(resolve("vercel", "us", true,  false, false), ActualRoute::Proxy("us".into()));
    }

    #[test]
    fn unknown_mode_fallback_chain_includes_static() {
        // Unknown / unrecognised route_mode (hand-edited JSON typo, stale
        // schema) must follow the same Proxy → Static → Vercel → Direct
        // chain so a configured staticProxy is never bypassed in favour
        // of Direct's real-IP exit.
        assert_eq!(resolve("bogus", "us", true,  false, false), ActualRoute::Proxy("us".into()));
        assert_eq!(resolve("bogus", "us", false, true,  false), ActualRoute::Static);
        assert_eq!(resolve("bogus", "us", false, true,  true),  ActualRoute::Static);
        assert_eq!(resolve("bogus", "us", false, false, true),  ActualRoute::Vercel);
        assert_eq!(resolve("bogus", "us", false, false, false), ActualRoute::Direct);
    }

    #[test]
    fn static_is_fallback_arm() {
        let r = resolve("static", "us", false, true, false);
        assert!(!r.is_fallback("static"));
        assert!(ActualRoute::Direct.is_fallback("static"));
    }

    #[test]
    fn static_label_text() {
        assert_eq!(ActualRoute::Static.label(), "Static");
    }

    #[test]
    fn old_json_compat() {
        assert_eq!(resolve("proxy", "us", true, false, false), ActualRoute::Proxy("us".into()));
    }

    #[test]
    fn unknown_mode_fallback_to_proxy() {
        assert_eq!(resolve("Proxy", "jp", true, false, false), ActualRoute::Proxy("jp".into()));
    }

    #[test]
    fn unknown_mode_fallback_to_vercel() {
        assert_eq!(resolve("INVALID", "us", false, false, true), ActualRoute::Vercel);
    }

    #[test]
    fn unknown_mode_fallback_to_direct() {
        assert_eq!(resolve("???", "us", false, false, false), ActualRoute::Direct);
    }

    #[test]
    fn is_fallback_proxy_to_vercel() {
        let r = resolve("proxy", "us", false, false, true);
        assert_eq!(r, ActualRoute::Vercel);
        assert!(r.is_fallback("proxy"));
        assert!(!r.is_fallback("vercel"));
    }

    #[test]
    fn label_format() {
        assert_eq!(ActualRoute::Proxy("jp".into()).label(), "Proxy/JP");
        assert_eq!(ActualRoute::Vercel.label(), "Vercel");
        assert_eq!(ActualRoute::Direct.label(), "Direct");
    }

    #[test]
    fn country_fallback_chain() {
        assert_eq!(resolve("proxy", "kr", true, false, false), ActualRoute::Proxy("kr".into()));
        assert_eq!(resolve("proxy", "jp", true, false, false), ActualRoute::Proxy("jp".into()));
        assert_eq!(resolve("proxy", "us", true, false, false), ActualRoute::Proxy("us".into()));
    }
}
