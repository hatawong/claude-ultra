#[derive(Debug, Clone, PartialEq)]
pub enum ActualRoute {
    Proxy(String),
    Vercel,
    Direct,
}

impl ActualRoute {
    pub fn is_fallback(&self, intended_mode: &str) -> bool {
        !matches!(
            (intended_mode, self),
            ("proxy", ActualRoute::Proxy(_)) | ("vercel", ActualRoute::Vercel) | ("direct", ActualRoute::Direct)
        )
    }

    pub fn label(&self) -> String {
        match self {
            ActualRoute::Proxy(c) => format!("Proxy/{}", c.to_uppercase()),
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
    has_vercel: bool,
) -> ActualRoute {
    let country = || proxy_country.to_lowercase();

    match route_mode {
        "proxy" => {
            if has_proxy {
                ActualRoute::Proxy(country())
            } else if has_vercel {
                ActualRoute::Vercel
            } else {
                ActualRoute::Direct
            }
        }
        "vercel" => {
            if has_vercel {
                ActualRoute::Vercel
            } else if has_proxy {
                ActualRoute::Proxy(country())
            } else {
                ActualRoute::Direct
            }
        }
        "direct" => ActualRoute::Direct,
        other => {
            tracing::warn!("unknown route_mode '{}', falling back to proxy chain", other);
            if has_proxy {
                ActualRoute::Proxy(country())
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

    fn resolve(mode: &str, country: &str, hp: bool, hv: bool) -> ActualRoute {
        resolve_route(mode, country, hp, hv)
    }

    #[test]
    fn proxy_normal() {
        assert_eq!(resolve("proxy", "jp", true, false), ActualRoute::Proxy("jp".into()));
    }

    #[test]
    fn proxy_fallback_vercel() {
        assert_eq!(resolve("proxy", "jp", false, true), ActualRoute::Vercel);
    }

    #[test]
    fn proxy_fallback_direct() {
        assert_eq!(resolve("proxy", "jp", false, false), ActualRoute::Direct);
    }

    #[test]
    fn vercel_normal() {
        assert_eq!(resolve("vercel", "us", false, true), ActualRoute::Vercel);
    }

    #[test]
    fn vercel_fallback_proxy() {
        assert_eq!(resolve("vercel", "us", true, false), ActualRoute::Proxy("us".into()));
    }

    #[test]
    fn vercel_fallback_direct() {
        assert_eq!(resolve("vercel", "us", false, false), ActualRoute::Direct);
    }

    #[test]
    fn direct_always() {
        assert_eq!(resolve("direct", "jp", true, true), ActualRoute::Direct);
    }

    #[test]
    fn old_json_compat() {
        assert_eq!(resolve("proxy", "us", true, false), ActualRoute::Proxy("us".into()));
    }

    #[test]
    fn unknown_mode_fallback_to_proxy() {
        assert_eq!(resolve("Proxy", "jp", true, false), ActualRoute::Proxy("jp".into()));
    }

    #[test]
    fn unknown_mode_fallback_to_vercel() {
        assert_eq!(resolve("INVALID", "us", false, true), ActualRoute::Vercel);
    }

    #[test]
    fn unknown_mode_fallback_to_direct() {
        assert_eq!(resolve("???", "us", false, false), ActualRoute::Direct);
    }

    #[test]
    fn is_fallback_proxy_to_vercel() {
        let r = resolve("proxy", "us", false, true);
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
        assert_eq!(resolve("proxy", "kr", true, false), ActualRoute::Proxy("kr".into()));
        assert_eq!(resolve("proxy", "jp", true, false), ActualRoute::Proxy("jp".into()));
        assert_eq!(resolve("proxy", "us", true, false), ActualRoute::Proxy("us".into()));
    }
}
