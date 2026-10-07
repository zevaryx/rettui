//! HTTPS for the web UI, when asked for at startup (`--https`, or
//! `--tls-cert` and `--tls-key`). Without either, the web UI is plain HTTP
//! as it always was.
//!
//! With `--https` alone rettui uses certificates of its own, kept in
//! `web-tls/` in the data directory: a small certificate authority, made
//! once, and a certificate for this computer's names and addresses that it
//! signs, made again when those change or it nears its end. A device that
//! installs the authority's certificate (served at `rettui-ca.crt`) then
//! trusts rettui's pages like any other secure site, which is what phones
//! need for notifications, installing the app, the camera and the
//! microphone.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::server::TlsStream;

/// How the web UI was asked to serve HTTPS.
#[derive(Debug, Clone, PartialEq)]
pub enum Https {
    /// With rettui's own certificate authority.
    Own,
    /// With a certificate (and its chain) and key from these PEM files.
    Files { cert: PathBuf, key: PathBuf },
}

/// What the web UI tells browsers and prints about its HTTPS.
#[derive(Debug, Clone)]
pub struct Served {
    pub config: Arc<ServerConfig>,
    /// rettui's own authority's certificate (DER), to install on devices.
    pub ca: Option<Arc<Vec<u8>>>,
    /// Its SHA-256 fingerprint, as browsers show it.
    pub fingerprint: Option<String>,
    /// The names and addresses rettui's own certificate is for.
    pub names: Vec<String>,
}

/// How long rettui's own certificates last: the authority for years, the
/// server's within what Apple's devices accept, made again a month before.
const CA_DAYS: i64 = 3650;
const SERVER_DAYS: i64 = 397;
const RENEW_DAYS: i64 = 30;
/// How long a TLS handshake (or a plain request on the HTTPS port) may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// What `cert.json` says of the server certificate beside it.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Issued {
    names: Vec<String>,
    /// Unix seconds.
    not_after: i64,
}

/// The TLS setup for `how`, with certificates read (or, rettui's own, made).
/// `listening` is the address the web UI listens on, for the names the
/// certificate covers.
pub fn prepare(how: &Https, dir: &Path, listening: IpAddr) -> Result<Served> {
    let (chain, key, ca, names) = match how {
        Https::Files { cert, key } => {
            let chain = CertificateDer::pem_file_iter(cert)
                .with_context(|| format!("could not read {}", cert.display()))?
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("{} isn't a PEM certificate", cert.display()))?;
            if chain.is_empty() {
                return Err(anyhow!("{} has no certificate in it", cert.display()));
            }
            let key = PrivateKeyDer::from_pem_file(key)
                .with_context(|| format!("could not read the private key in {}", key.display()))?;
            (chain, key, None, Vec::new())
        }
        Https::Own => {
            let names = names_for(listening);
            let own = own_certificates(dir, &names)?;
            (own.chain, own.key, Some(own.ca), names)
        }
    };
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS versions")?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .context("the certificate and key don't go together")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let fingerprint = ca.as_ref().map(|der| fingerprint(der));
    Ok(Served { config: Arc::new(config), ca: ca.map(Arc::new), fingerprint, names })
}

/// SHA-256 of a certificate, in pairs of hex digits.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(der);
    digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Names and addresses this computer is reached at: localhost, its host
/// name (and `.local`, as mDNS has it), the address listened on, and the
/// addresses it reaches the network from.
fn names_for(listening: IpAddr) -> Vec<String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
    if let Some(host) = host_name() {
        let host = host.to_ascii_lowercase();
        if !host.contains('.') {
            names.push(format!("{host}.local"));
        }
        names.push(host);
    }
    if !listening.is_unspecified() && !listening.is_loopback() {
        names.push(listening.to_string());
    }
    let outgoing = |bind: &str, to: &str| {
        let socket = std::net::UdpSocket::bind(bind).ok()?;
        socket.connect(to).ok()?;
        Some(socket.local_addr().ok()?.ip())
    };
    // Connecting a UDP socket sends nothing: it only picks the route.
    for ip in [outgoing("0.0.0.0:0", "192.0.2.1:9"), outgoing("[::]:0", "[2001:db8::1]:9")].into_iter().flatten() {
        if !ip.is_loopback() && !ip.is_unspecified() {
            names.push(ip.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length, which gethostname is told.
    let done = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if done != 0 {
        return None;
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    let name = std::str::from_utf8(&buffer[..end]).ok()?.trim().to_string();
    valid_dns_name(&name).then_some(name)
}

#[cfg(not(unix))]
fn host_name() -> Option<String> {
    let name = std::env::var("COMPUTERNAME").ok()?.trim().to_string();
    valid_dns_name(&name).then_some(name)
}

fn valid_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

struct Own {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    ca: Vec<u8>,
}

/// rettui's authority (made the first time) and a server certificate it
/// signed for `names` (made again when they change or it nears its end).
fn own_certificates(dir: &Path, names: &[String]) -> Result<Own> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not make {}", dir.display()))?;
    let (ca_key_path, ca_path) = (dir.join("ca.key"), dir.join("ca.pem"));
    let (key_path, cert_path, issued_path) = (dir.join("cert.key"), dir.join("cert.pem"), dir.join("cert.json"));
    let ca_params = authority_params();
    let ca_key = match std::fs::read_to_string(&ca_key_path) {
        Ok(pem) if ca_path.exists() => KeyPair::from_pem(&pem).context("rettui's certificate authority key")?,
        _ => {
            let key = KeyPair::generate().context("making a key")?;
            let cert = ca_params.self_signed(&key).context("making rettui's certificate authority")?;
            crate::config::write_private(&ca_key_path, key.serialize_pem().as_bytes())?;
            std::fs::write(&ca_path, cert.pem())?;
            // A new authority: the old server certificate is no longer its.
            let _ = std::fs::remove_file(&issued_path);
            key
        }
    };
    let ca_der = CertificateDer::from_pem_file(&ca_path).context("rettui's certificate authority")?;

    let now = chrono::Utc::now().timestamp();
    let issued: Option<Issued> = std::fs::read(&issued_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let current = issued.is_some_and(|i| i.names == names && i.not_after - now > RENEW_DAYS * 86_400)
        && key_path.exists()
        && cert_path.exists();
    if !current {
        let key = KeyPair::generate().context("making a key")?;
        let mut params = CertificateParams::new(names.to_vec()).context("this computer's names")?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "rettui web UI");
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.use_authority_key_identifier_extension = true;
        let not_after = now + SERVER_DAYS * 86_400;
        (params.not_before, params.not_after) = (date(now - 86_400), date(not_after));
        let issuer = Issuer::new(ca_params, &ca_key);
        let cert = params.signed_by(&key, &issuer).context("signing the certificate")?;
        crate::config::write_private(&key_path, key.serialize_pem().as_bytes())?;
        std::fs::write(&cert_path, cert.pem())?;
        std::fs::write(&issued_path, serde_json::to_vec_pretty(&Issued { names: names.to_vec(), not_after })?)?;
    }
    let cert = CertificateDer::from_pem_file(&cert_path).context("rettui's certificate")?;
    let key = PrivateKeyDer::from_pem_file(&key_path).context("rettui's certificate key")?;
    Ok(Own { chain: vec![cert, ca_der.clone()], key, ca: ca_der.to_vec() })
}

/// The authority's name and uses: the same each start, so it can sign
/// again with its saved key.
fn authority_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "rettui local certificate authority");
    params.distinguished_name.push(DnType::OrganizationName, "rettui");
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    let now = chrono::Utc::now().timestamp();
    (params.not_before, params.not_after) = (date(now - 86_400), date(now + CA_DAYS * 86_400));
    params
}

fn date(unix: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(unix).unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

/// Connections that finished their TLS handshake, for axum to serve. Each
/// handshake runs on its own, so a slow one doesn't hold up the rest; a
/// plain HTTP request on the HTTPS port is sent to its `https://` address.
pub struct TlsListener {
    connections: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(listener: TcpListener, config: Arc<ServerConfig>) -> Result<Self> {
        let local = listener.local_addr()?;
        let acceptor = TlsAcceptor::from(config);
        let (done, connections) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                let (tcp, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        tracing::debug!("web UI accept: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let (acceptor, done) = (acceptor.clone(), done.clone());
                tokio::spawn(async move {
                    let mut first = [0u8; 1];
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, tcp.peek(&mut first)).await {
                        // A TLS handshake starts with 0x16; anything else is
                        // plain HTTP (or nothing worth answering).
                        Ok(Ok(1)) if first[0] != 0x16 => {
                            let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, to_https(tcp)).await;
                        }
                        Ok(Ok(1)) => {
                            if let Ok(Ok(stream)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                                let _ = done.send((stream, peer)).await;
                            }
                        }
                        _ => {}
                    }
                });
            }
        });
        Ok(Self { connections, local })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.connections.recv().await {
            Some(connection) => connection,
            // The accepting task is gone: nothing more will come.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Answer a plain HTTP request with a redirect to the same page over HTTPS.
async fn to_https(mut tcp: TcpStream) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 8192 {
        let read = tcp.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let response = match https_location(&String::from_utf8_lossy(&request)) {
        Some(location) => format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        None => "HTTP/1.1 400 Bad Request\r\nContent-Length: 30\r\nConnection: close\r\n\r\nThis is an HTTPS address only".into(),
    };
    tcp.write_all(response.as_bytes()).await?;
    tcp.shutdown().await
}

/// Where a plain HTTP request should have gone: `https://` its host and path.
fn https_location(request: &str) -> Option<String> {
    let mut lines = request.split("\r\n");
    let path = lines.next()?.split(' ').nth(1).filter(|p| p.starts_with('/'))?;
    let host = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("host").then(|| value.trim())
    })?;
    let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic());
    (safe(host) && safe(path)).then(|| format!("https://{host}{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_requests_are_sent_to_https() {
        assert_eq!(
            https_location("GET /?token=abc HTTP/1.1\r\nHost: 192.168.1.5:8740\r\nAccept: */*\r\n\r\n").as_deref(),
            Some("https://192.168.1.5:8740/?token=abc")
        );
        assert_eq!(https_location("GET / HTTP/1.1\r\n\r\n"), None);
        assert_eq!(https_location("GET / HTTP/1.1\r\nHost: evil\r\n x\r\n\r\n").as_deref(), Some("https://evil/"));
        assert_eq!(https_location("GET / HTTP/1.1\r\nHost: a b\r\n\r\n"), None);
    }

    #[test]
    fn host_names_are_checked() {
        assert!(valid_dns_name("raspberrypi"));
        assert!(valid_dns_name("my-host.lan"));
        assert!(!valid_dns_name("-bad"));
        assert!(!valid_dns_name("has space"));
        assert!(!valid_dns_name(""));
    }

    #[test]
    fn rettuis_own_certificates_are_made_kept_and_renewed() {
        let dir = std::env::temp_dir().join(format!("rettui-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let names = vec!["127.0.0.1".to_string(), "localhost".to_string()];
        let first = own_certificates(&dir, &names).unwrap();
        assert_eq!(first.chain.len(), 2);
        // The next start keeps both.
        let again = own_certificates(&dir, &names).unwrap();
        assert_eq!((again.ca.clone(), again.chain[0].to_vec()), (first.ca.clone(), first.chain[0].to_vec()));
        // A new address: a new server certificate from the same authority.
        let moved = own_certificates(&dir, &[names.clone(), vec!["192.168.1.5".into()]].concat()).unwrap();
        assert_eq!(moved.ca, first.ca);
        assert_ne!(moved.chain[0].to_vec(), first.chain[0].to_vec());
        // Nearing its end: made again.
        let issued = dir.join("cert.json");
        let mut stale: Issued = serde_json::from_slice(&std::fs::read(&issued).unwrap()).unwrap();
        stale.not_after = chrono::Utc::now().timestamp() + 86_400;
        let names = stale.names.clone();
        std::fs::write(&issued, serde_json::to_vec(&stale).unwrap()).unwrap();
        let renewed = own_certificates(&dir, &names).unwrap();
        assert_ne!(renewed.chain[0].to_vec(), moved.chain[0].to_vec());
        // The keys are private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for key in ["ca.key", "cert.key"] {
                assert_eq!(std::fs::metadata(dir.join(key)).unwrap().permissions().mode() & 0o077, 0, "{key}");
            }
        }
        // And rustls takes them.
        let served = prepare(&Https::Own, &dir, "127.0.0.1".parse().unwrap()).unwrap();
        assert!(served.ca.is_some() && served.fingerprint.unwrap().len() == 95);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
