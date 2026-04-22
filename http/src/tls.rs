//! Stub TLS config builder. Returns a plain BoringSSL client connector;
//! source builds cannot produce valid upstream requests.

use boring::ssl::{SslConnector, SslConnectorBuilder, SslMethod};

pub fn build_ssl_config() -> SslConnectorBuilder {
    SslConnector::builder(SslMethod::tls())
        .expect("SslConnector::builder must not fail")
}
