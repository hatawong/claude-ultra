//! Stub license module.
//! Real JWT / RS256 verification and concurrency guard ship only in the
//! official .dmg build. Stub preserves the public API so Manager compiles;
//! all license checks succeed.

#[derive(Debug, Clone)]
pub enum LicenseError {
    NoLicense,
    LicenseExpired,
    LicenseInvalid(String),
    ConcurrencyLimitExceeded { max: u32, active: usize },
}

impl std::fmt::Display for LicenseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseError::NoLicense => write!(f, "No license"),
            LicenseError::LicenseExpired => write!(f, "License expired"),
            LicenseError::LicenseInvalid(msg) => write!(f, "Invalid license: {}", msg),
            LicenseError::ConcurrencyLimitExceeded { max, active } => {
                write!(f, "Concurrency limit exceeded (max: {}, active: {})", max, active)
            }
        }
    }
}

pub struct LicenseGuard {
    plan: String,
    max: u32,
}

impl LicenseGuard {
    /// Stub accepts any JWT, returns a permissive guard.
    pub fn from_jwt(_jwt: &str) -> Result<Self, LicenseError> {
        Ok(Self { plan: "stub".to_string(), max: 0 })
    }

    /// Stub always passes concurrency checks.
    pub fn check(&self, _account_uuid: &str) -> Result<(), LicenseError> {
        Ok(())
    }

    pub fn plan(&self) -> &str {
        &self.plan
    }

    pub fn max_accounts(&self) -> u32 {
        self.max
    }
}

/// Stub set_license: always Ok.
pub fn set_license(_jwt: &str) -> Result<(), LicenseError> {
    Ok(())
}

/// Stub clear_license: no-op.
pub fn clear_license() {}

/// Stub check_license: always Ok.
pub fn check_license(_account_uuid: &str) -> Result<(), LicenseError> {
    Ok(())
}
