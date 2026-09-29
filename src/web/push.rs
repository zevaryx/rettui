//! Web Push: notifications for browsers that aren't showing the web UI, such
//! as a phone with rettui's app in the background (the phone soon stops its
//! live connection).
//!
//! A browser that turns it on (Status > Notifications) gives rettui a
//! subscription: an address at its push service (Google's, Mozilla's,
//! Apple's or Microsoft's) and keys. Each notification is sent there
//! encrypted for that browser alone (RFC 8291), signed with this server's
//! key (VAPID, RFC 8292). The push service sees when one is sent, not what
//! it says. Browsers showing the web UI get notifications over their live
//! connection instead, and aren't pushed to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::app::notify::Notification;
use crate::config::Paths;

/// Push services a subscription may be at. rettui sends requests only
/// there: a subscription could name any address, and must not make rettui
/// send requests into the network it runs in.
const PUSH_SERVICES: [&str; 4] = ["fcm.googleapis.com", "push.services.mozilla.com", "push.apple.com", "notify.windows.com"];
/// How long a push service keeps a notification for a browser that's offline.
const TTL_SECS: u64 = 24 * 3600;
/// How long a browser that said it shows the web UI counts as showing it
/// (it says so again every minute while it does).
const SHOWING_FOR: Duration = Duration::from_secs(90);
/// Who runs this server, for push services (VAPID's `sub`).
const CONTACT: &str = crate::config::PROJECT_URL;

/// A browser's push subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    /// The browser's key: P-256, uncompressed, base64url.
    pub p256dh: String,
    /// Its authentication secret: 16 bytes, base64url.
    pub auth: String,
}

impl Subscription {
    /// Checked: an HTTPS address at a known push service, and keys of the
    /// right sizes.
    pub fn checked(self) -> Result<Self, String> {
        push_service(&self.endpoint).ok_or("Not a push service rettui knows (Google, Mozilla, Apple or Microsoft)")?;
        let key = B64.decode(self.p256dh.trim_end_matches('=')).map_err(|_| "The browser's key isn't base64url")?;
        PublicKey::from_sec1_bytes(&key).map_err(|_| "The browser's key isn't a P-256 key")?;
        let auth = B64.decode(self.auth.trim_end_matches('=')).map_err(|_| "The authentication secret isn't base64url")?;
        if auth.len() != 16 {
            return Err("The authentication secret isn't 16 bytes".into());
        }
        Ok(self)
    }
}

/// The push service an endpoint is at (`https://` and a known host), for
/// VAPID's audience: `https://host`.
fn push_service(endpoint: &str) -> Option<String> {
    let rest = endpoint.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    // No port, user or other oddities: just a host name.
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    PUSH_SERVICES
        .iter()
        .any(|service| host == *service || host.ends_with(&format!(".{service}")))
        .then(|| format!("https://{host}"))
}

/// `payload` encrypted for one browser (RFC 8291, `aes128gcm`), from the
/// sender key pair `ours` and `salt` (new for each message).
fn encrypt(payload: &[u8], theirs: &[u8], auth: &[u8], ours: &SecretKey, salt: &[u8; 16]) -> Result<Vec<u8>, String> {
    let their_key = PublicKey::from_sec1_bytes(theirs).map_err(|_| "bad browser key")?;
    let our_public = ours.public_key().to_encoded_point(false);
    let shared = p256::ecdh::diffie_hellman(ours.to_nonzero_scalar(), their_key.as_affine());
    // The input keying material, from the shared secret and the browser's
    // authentication secret.
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(theirs);
    info.extend_from_slice(our_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .map_err(|_| "key derivation failed")?;
    // The content encryption key and nonce, from it and the salt.
    let hkdf = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let (mut key, mut nonce) = ([0u8; 16], [0u8; 12]);
    hkdf.expand(b"Content-Encoding: aes128gcm\0", &mut key).map_err(|_| "key derivation failed")?;
    hkdf.expand(b"Content-Encoding: nonce\0", &mut nonce).map_err(|_| "key derivation failed")?;
    // One record: the payload, then the last-record delimiter.
    let mut record = payload.to_vec();
    record.push(2);
    let sealed = Aes128Gcm::new(&key.into())
        .encrypt(&nonce.into(), record.as_slice())
        .map_err(|_| "encryption failed")?;
    // Header: salt, record size, and our public key as the key id.
    let mut body = Vec::with_capacity(21 + 65 + sealed.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&4096u32.to_be_bytes());
    body.push(65);
    body.extend_from_slice(our_public.as_bytes());
    body.extend_from_slice(&sealed);
    Ok(body)
}

/// The `Authorization` header for a push service (VAPID): a token for
/// `audience`, signed with this server's key, and that key.
fn vapid(key: &SigningKey, public: &str, audience: &str, now: u64) -> String {
    let header = B64.encode(r#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(json!({ "aud": audience, "exp": now + 12 * 3600, "sub": CONTACT }).to_string());
    let signed = format!("{header}.{claims}");
    let signature: Signature = key.sign(signed.as_bytes());
    format!("vapid t={signed}.{}, k={public}", B64.encode(signature.to_bytes()))
}

/// A push service's topic for what a notification is about: one waiting to
/// be delivered is replaced by a newer one about the same conversation or
/// room (32 base64url characters, as the header allows).
fn topic(tag: &str) -> String {
    B64.encode(&Sha256::digest(tag.as_bytes())[..24])
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

struct Push {
    subscription: Subscription,
    payload: Vec<u8>,
    topic: String,
}

/// Subscriptions and sending (on a thread of its own: push services can be
/// slow, or out of reach on a network without the internet).
#[derive(Clone)]
pub struct WebPush {
    /// This server's public key (P-256, uncompressed, base64url), which
    /// browsers subscribe with.
    public: String,
    path: PathBuf,
    subscriptions: Arc<Mutex<Vec<Subscription>>>,
    /// Browsers showing the web UI (by endpoint), until when.
    showing: Arc<Mutex<HashMap<String, Instant>>>,
    queue: mpsc::Sender<Push>,
    reports: Arc<Mutex<mpsc::Receiver<String>>>,
}

impl WebPush {
    /// This server's key (created on first use) and the subscriptions saved.
    pub fn load(paths: &Paths) -> Result<Self> {
        let secret = match std::fs::read_to_string(&paths.web_push_key) {
            Ok(text) => SecretKey::from_slice(&hex::decode(text.trim())?)?,
            Err(_) => {
                let secret = SecretKey::random(&mut rand::rngs::OsRng);
                crate::config::write_private(&paths.web_push_key, hex::encode(secret.to_bytes()).as_bytes())?;
                secret
            }
        };
        let public = B64.encode(secret.public_key().to_encoded_point(false).as_bytes());
        let subscriptions: Vec<Subscription> = std::fs::read(&paths.web_push)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let subscriptions = Arc::new(Mutex::new(subscriptions));
        let (queue, pushes) = mpsc::channel::<Push>();
        let (report, reports) = mpsc::channel();
        let sender = Sender {
            key: SigningKey::from(&secret),
            public: public.clone(),
            path: paths.web_push.clone(),
            subscriptions: subscriptions.clone(),
            report,
        };
        std::thread::Builder::new().name("rettui-push".into()).spawn(move || sender.run(pushes))?;
        Ok(Self {
            public,
            path: paths.web_push.clone(),
            subscriptions,
            showing: Arc::new(Mutex::new(HashMap::new())),
            queue,
            reports: Arc::new(Mutex::new(reports)),
        })
    }

    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// Add a browser (or update it: the same endpoint again replaces it).
    pub fn subscribe(&self, subscription: Subscription) -> Result<(), String> {
        let subscription = subscription.checked()?;
        let mut subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.retain(|s| s.endpoint != subscription.endpoint);
        subscriptions.push(subscription);
        save(&self.path, &subscriptions)
    }

    pub fn unsubscribe(&self, endpoint: &str) -> Result<(), String> {
        let mut subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.retain(|s| s.endpoint != endpoint);
        self.showing.lock().unwrap().remove(endpoint);
        save(&self.path, &subscriptions)
    }

    /// Whether the browser with this subscription shows the web UI now.
    pub fn set_showing(&self, endpoint: &str, showing: bool) {
        let mut browsers = self.showing.lock().unwrap();
        if showing {
            browsers.insert(endpoint.to_string(), Instant::now() + SHOWING_FOR);
        } else {
            browsers.remove(endpoint);
        }
    }

    /// Push a notification to every browser subscribed that isn't showing
    /// the web UI.
    pub fn notify(&self, notification: &Notification) {
        let tag = notification.target.tag();
        let payload = json!({
            "title": notification.title,
            "body": notification.body,
            "target": notification.target,
            "tag": tag,
        })
        .to_string()
        .into_bytes();
        let now = Instant::now();
        let mut showing = self.showing.lock().unwrap();
        showing.retain(|_, until| *until > now);
        for subscription in self.subscriptions.lock().unwrap().iter() {
            if showing.contains_key(&subscription.endpoint) {
                continue;
            }
            let _ = self.queue.send(Push { subscription: subscription.clone(), payload: payload.clone(), topic: topic(&tag) });
        }
    }

    /// What sending had to say, for the log.
    pub fn reports(&self) -> Vec<String> {
        self.reports.lock().unwrap().try_iter().collect()
    }
}

fn save(path: &Path, subscriptions: &[Subscription]) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(subscriptions).map_err(|e| e.to_string())?;
    crate::config::write_private(path, &json).map_err(|e| format!("Could not save {}: {e}", path.display()))
}

struct Sender {
    key: SigningKey,
    public: String,
    path: PathBuf,
    subscriptions: Arc<Mutex<Vec<Subscription>>>,
    report: mpsc::Sender<String>,
}

impl Sender {
    fn run(self, pushes: mpsc::Receiver<Push>) {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .try_proxy_from_env(true)
            .build();
        // Out of reach (no internet, say) is said once, until it works again.
        let mut unreachable = false;
        while let Ok(push) = pushes.recv() {
            match self.send(&agent, &push) {
                Ok(()) => unreachable = false,
                // The browser unsubscribed, or its subscription expired.
                Err(Failure::Gone) => {
                    let mut subscriptions = self.subscriptions.lock().unwrap();
                    subscriptions.retain(|s| s.endpoint != push.subscription.endpoint);
                    let _ = save(&self.path, &subscriptions);
                    let _ = self.report.send("A browser's background notifications ended (its subscription expired); turn them on again there".into());
                }
                Err(Failure::Refused(e)) => {
                    tracing::warn!("push refused: {e}");
                    let _ = self.report.send(format!("A push service refused a notification: {e}"));
                }
                Err(Failure::Unreachable(e)) => {
                    tracing::warn!("push failed: {e}");
                    if !unreachable {
                        unreachable = true;
                        let _ = self.report.send(format!("Could not reach a push service for background notifications: {e}"));
                    }
                }
            }
        }
    }

    fn send(&self, agent: &ureq::Agent, push: &Push) -> Result<(), Failure> {
        let subscription = &push.subscription;
        let audience = push_service(&subscription.endpoint).ok_or(Failure::Gone)?;
        let theirs = B64.decode(subscription.p256dh.trim_end_matches('=')).map_err(|_| Failure::Gone)?;
        let auth = B64.decode(subscription.auth.trim_end_matches('=')).map_err(|_| Failure::Gone)?;
        let salt: [u8; 16] = rand::random();
        let body = encrypt(&push.payload, &theirs, &auth, &SecretKey::random(&mut rand::rngs::OsRng), &salt)
            .map_err(|e| Failure::Refused(e.to_string()))?;
        let result = agent
            .post(&subscription.endpoint)
            .set("Authorization", &vapid(&self.key, &self.public, &audience, now()))
            .set("Content-Encoding", "aes128gcm")
            .set("Content-Type", "application/octet-stream")
            .set("TTL", &TTL_SECS.to_string())
            .set("Urgency", "high")
            .set("Topic", &push.topic)
            .send_bytes(&body);
        match result {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(404 | 410, _)) => Err(Failure::Gone),
            Err(ureq::Error::Status(code, response)) => {
                let text = response.into_string().unwrap_or_default();
                Err(Failure::Refused(format!("{code} {}", text.chars().take(200).collect::<String>())))
            }
            Err(e) => Err(Failure::Unreachable(e.to_string())),
        }
    }
}

enum Failure {
    Gone,
    Refused(String),
    Unreachable(String),
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    use super::*;

    fn b64(text: &str) -> Vec<u8> {
        B64.decode(text).unwrap()
    }

    /// RFC 8291, section 5: the worked example, byte for byte.
    #[test]
    fn encrypts_as_rfc_8291_does() {
        let ours = SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        assert_eq!(
            B64.encode(ours.public_key().to_encoded_point(false).as_bytes()),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let theirs = b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt(b"When I grow up, I want to be a watermelon", &theirs, &auth, &ours, &salt).unwrap();
        assert_eq!(
            B64.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn vapid_tokens_verify_with_the_public_key() {
        let secret = SecretKey::random(&mut rand::rngs::OsRng);
        let key = SigningKey::from(&secret);
        let public = B64.encode(secret.public_key().to_encoded_point(false).as_bytes());
        let header = vapid(&key, &public, "https://fcm.googleapis.com", 1_800_000_000);
        let (token, k) = header.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(k, public);
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&b64(signed.split('.').nth(1).unwrap())).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_800_000_000 + 12 * 3600);
        let verifying = VerifyingKey::from_sec1_bytes(&b64(&public)).unwrap();
        let signature = Signature::from_slice(&b64(signature)).unwrap();
        assert!(verifying.verify(signed.as_bytes(), &signature).is_ok());
    }

    #[test]
    fn only_known_push_services_over_https() {
        assert_eq!(push_service("https://fcm.googleapis.com/fcm/send/abc").as_deref(), Some("https://fcm.googleapis.com"));
        assert_eq!(
            push_service("https://updates.push.services.mozilla.com/wpush/v2/x").as_deref(),
            Some("https://updates.push.services.mozilla.com")
        );
        assert!(push_service("https://web.push.apple.com/QK").is_some());
        assert!(push_service("https://wns2-par02p.notify.windows.com/w/?token=x").is_some());
        for bad in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/x",
            "https://fcm.googleapis.com.evil.example/x",
            "https://evilfcm.googleapis.com.example/x",
            "https://user@fcm.googleapis.com/x",
            "https://fcm.googleapis.com:8443/x",
            "https://notfcm-googleapis.com/x",
        ] {
            assert!(push_service(bad).is_none(), "{bad}");
        }
        let good = Subscription {
            endpoint: "https://fcm.googleapis.com/fcm/send/abc".into(),
            p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4".into(),
            auth: "BTBZMqHH6r4Tts7J_aSIgg".into(),
        };
        assert!(good.clone().checked().is_ok());
        assert!(Subscription { auth: "AAAA".into(), ..good.clone() }.checked().is_err());
        assert!(Subscription { p256dh: "BAAA".into(), ..good.clone() }.checked().is_err());
        assert!(Subscription { endpoint: "https://10.0.0.1/x".into(), ..good }.checked().is_err());
    }

    #[test]
    fn topics_fit_the_header() {
        let t = topic(&format!("lxmf:{}", "ab".repeat(16)));
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(t, topic("rrc:x:general"));
    }
}
