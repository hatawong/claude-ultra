pub mod error;
pub mod tls;
pub mod client;
pub mod decode;
pub mod license;

pub use client::{BoringClient, BoringClientBuilder, ConnectTimings, RequestBuilder, UpstreamClient};
pub use decode::{decode_response, DecodeError, DecodedBody};
pub use error::{ConnectStep, Error};
pub use license::LicenseError;

/// Stub: returns a constant.
pub fn compute_fp(_a: &str, _b: &str) -> String {
    "000".to_string()
}

/// A `Result` alias where the `Err` case is `claude_ultra_http::Error`.
pub type Result<T> = std::result::Result<T, Error>;
