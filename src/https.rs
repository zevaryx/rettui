//! rettui's own HTTPS requests (Web Push, map tiles, update checks): what
//! they trust, and the proxy they go through.
//!
//! They trust the root certificates rettui comes with and this computer's
//! own (its system store, or `SSL_CERT_FILE` / `SSL_CERT_DIR`), as
//! browsers do: so they work behind a proxy that inspects HTTPS with a
//! certificate authority of its own (an office's, or antivirus), and on a
//! system with no store at all (a small container image).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// An agent for HTTPS requests: trusting both sets of roots, through the
/// proxy set in the environment if any, giving up after `timeout`.
pub fn agent(timeout: Duration) -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().timeout(timeout).try_proxy_from_env(true).tls_config(config())
}

/// Made once: the system store is read once a run.
fn config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("ring supports the default TLS versions")
                .with_root_certificates(roots())
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// The bundled roots, and this computer's (those that can't be read are
/// skipped: the bundled ones still work).
fn roots() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let native = rustls_native_certs::load_native_certs();
    for e in &native.errors {
        tracing::debug!("couldn't read some of the system's certificates: {e}");
    }
    let (added, skipped) = roots.add_parsable_certificates(native.certs);
    tracing::debug!(
        "trusting {} bundled root certificates and {added} of the system's ({skipped} skipped)",
        webpki_roots::TLS_SERVER_ROOTS.len()
    );
    roots
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_bundled_roots_are_always_there() {
        // Whatever the system has, the bundled roots are trusted.
        assert!(super::roots().len() >= webpki_roots::TLS_SERVER_ROOTS.len());
        // Made once.
        assert!(std::sync::Arc::ptr_eq(&super::config(), &super::config()));
    }
}
