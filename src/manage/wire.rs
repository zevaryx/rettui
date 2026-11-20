//! How a management request and its answer are signed: the text each
//! signature covers, HMAC-SHA256 with the node's secret, and the headers a
//! request carries. The website signs with the same rules (see the test
//! vector below, which both sides check against).

use std::sync::Arc;

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// The request's headers, as HTTP gives them (lower case).
pub const TIMESTAMP: &str = "x-rettui-timestamp";
pub const NONCE: &str = "x-rettui-nonce";
pub const ACTOR: &str = "x-rettui-actor";
/// On a request and on an answer: `v1=` and 64 lowercase hex characters.
pub const SIGNATURE: &str = "x-rettui-signature";

/// The shortest and longest secret accepted.
pub const SECRET_MIN: usize = 32;
pub const SECRET_MAX: usize = 256;

/// The secret the node and the website share. It is never printed: `Debug`
/// shows `Secret(..)`, and its bytes are wiped when the last copy goes.
#[derive(Clone)]
pub struct Secret(Arc<Zeroizing<Vec<u8>>>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// A secret from `text`: 32 to 256 printable ASCII characters, with no
    /// spaces, quotes or backslashes (as the stats token). The error never
    /// repeats the text.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() < SECRET_MIN {
            return Err(format!("the manage secret must be at least {SECRET_MIN} characters"));
        }
        if text.len() > SECRET_MAX {
            return Err(format!("the manage secret must be at most {SECRET_MAX} characters"));
        }
        if !text.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\'' | b'\\')) {
            return Err("the manage secret must be printable ASCII with no spaces, quotes or backslashes".into());
        }
        Ok(Self(Arc::new(Zeroizing::new(text.as_bytes().to_vec()))))
    }

    fn mac(&self) -> HmacSha256 {
        HmacSha256::new_from_slice(&self.0).expect("HMAC takes a key of any length")
    }

    /// Whether the secret is the stats token too, compared by digest (in
    /// the same time either way) and never shown.
    pub fn is_token(&self, token: &crate::stats::Token) -> bool {
        std::str::from_utf8(&self.0).is_ok_and(|text| token.matches(text))
    }

    /// Whether `text` is in the secret, for tests that check it never leaks.
    #[cfg(test)]
    pub fn is_in(&self, text: &str) -> bool {
        std::str::from_utf8(&self.0).is_ok_and(|secret| text.contains(secret))
    }
}

/// The SHA-256 of `body`, in lowercase hex.
pub fn body_hash(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// What a request's signature covers. `path_and_query` is the request
/// target as sent, and a request with no body (a GET) hashes "".
pub fn canonical(timestamp: &str, nonce: &str, method: &str, path_and_query: &str, actor: &str, body: &[u8]) -> String {
    format!("v1\n{timestamp}\n{nonce}\n{method}\n{path_and_query}\n{actor}\n{}", body_hash(body))
}

/// What an answer's signature covers: the request's nonce ties it to the
/// request, so an answer can't be replayed for another one.
pub fn response_canonical(nonce: &str, status: u16, body: &[u8]) -> String {
    format!("v1-response\n{nonce}\n{status}\n{}", body_hash(body))
}

/// The HMAC of `text`, in lowercase hex.
pub fn sign(secret: &Secret, text: &str) -> String {
    let mut mac = secret.mac();
    mac.update(text.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Whether `signature` (hex) is the HMAC of `text`, compared in constant time.
pub fn verify(secret: &Secret, text: &str, signature: &str) -> bool {
    let Ok(bytes) = hex::decode(signature) else { return false };
    let mut mac = secret.mac();
    mac.update(text.as_bytes());
    mac.verify_slice(&bytes).is_ok()
}

/// The `X-Rettui-Signature` value of an answer.
pub fn response_signature(secret: &Secret, nonce: &str, status: u16, body: &[u8]) -> String {
    format!("v1={}", sign(secret, &response_canonical(nonce, status, body)))
}

/// A request's signing headers, checked for form only (not yet for the signature).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// As sent, which is what was signed.
    timestamp_text: String,
    pub timestamp: u64,
    pub nonce: String,
    pub actor: String,
    signature: String,
}

/// The one value of header `name`, as text. None if it's missing, not text,
/// or given more than once (two would leave it open which one was signed).
fn one<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

fn lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `actor` is a name the website may give: a letter or digit, then
/// up to 31 letters, digits, `.`, `_` or `-`.
pub fn valid_actor(actor: &str) -> bool {
    let mut bytes = actor.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && actor.len() <= 32
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl Signed {
    /// The signing headers in `headers`, if each is there once and well formed.
    pub fn parse(headers: &HeaderMap) -> Option<Self> {
        let timestamp_text = one(headers, TIMESTAMP)?;
        if timestamp_text.is_empty() || timestamp_text.len() > 11 || !timestamp_text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let timestamp = timestamp_text.parse().ok()?;
        let nonce = one(headers, NONCE).filter(|n| lower_hex(n, 32))?;
        let actor = one(headers, ACTOR).filter(|a| valid_actor(a))?;
        let signature = one(headers, SIGNATURE)?.strip_prefix("v1=").filter(|s| lower_hex(s, 64))?;
        Some(Self {
            timestamp_text: timestamp_text.to_string(),
            timestamp,
            nonce: nonce.to_string(),
            actor: actor.to_string(),
            signature: signature.to_string(),
        })
    }

    /// Whether the signature is the secret's, over this request.
    pub fn verify(&self, secret: &Secret, method: &str, path_and_query: &str, body: &[u8]) -> bool {
        let text = canonical(&self.timestamp_text, &self.nonce, method, path_and_query, &self.actor, body);
        verify(secret, &text, &self.signature)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    // The test vector the website's tests check too: computed once with
    // openssl from the canonical strings, so both sides agree on one value.
    const SECRET: &str = "rettui-manage-test-secret-0123456789abcdef";
    const TS: &str = "1763640000";
    const NONCE_HEX: &str = "00112233445566778899aabbccddeeff";
    const BODY: &str = r#"{"revision":"00000000000000000000000000000000","changes":{"pn_name":"Hilltop"}}"#;
    const BODY_HASH: &str = "351153819104d13d6c1a8035173afe627c28812aaf1800cc294f20a537bfe4c0";
    const REQUEST_SIGNATURE: &str = "v1=e67c8c58195470e27718090f17e8615c1197e216a7c714ef6e9228aa84fccce7";
    const RESPONSE_SIGNATURE: &str = "v1=1f2f4a3b7b5ceaea194df98320fcf931e548b9fc21387d02aad45a11aa990ef2";

    fn secret() -> Secret {
        Secret::parse(SECRET).unwrap()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            // Bytes, so values that aren't ASCII can be sent too.
            map.append(*name, HeaderValue::from_bytes(value.as_bytes()).unwrap());
        }
        map
    }

    fn vector_headers() -> HeaderMap {
        headers(&[(TIMESTAMP, TS), (NONCE, NONCE_HEX), (ACTOR, "alice"), (SIGNATURE, REQUEST_SIGNATURE)])
    }

    #[test]
    fn the_request_vector_verifies() {
        assert_eq!(body_hash(BODY.as_bytes()), BODY_HASH);
        let text = canonical(TS, NONCE_HEX, "POST", "/manage/v1/settings", "alice", BODY.as_bytes());
        assert_eq!(format!("v1={}", sign(&secret(), &text)), REQUEST_SIGNATURE);
        let signed = Signed::parse(&vector_headers()).unwrap();
        assert_eq!((signed.timestamp, signed.nonce.as_str(), signed.actor.as_str()), (1_763_640_000, NONCE_HEX, "alice"));
        assert!(signed.verify(&secret(), "POST", "/manage/v1/settings", BODY.as_bytes()));
    }

    #[test]
    fn the_response_vector_verifies() {
        assert_eq!(response_signature(&secret(), NONCE_HEX, 200, br#"{"id":"x"}"#), RESPONSE_SIGNATURE);
        let hex = RESPONSE_SIGNATURE.strip_prefix("v1=").unwrap();
        assert!(verify(&secret(), &response_canonical(NONCE_HEX, 200, br#"{"id":"x"}"#), hex));
        // Another status, nonce or body is another signature.
        assert_ne!(response_signature(&secret(), NONCE_HEX, 201, br#"{"id":"x"}"#), RESPONSE_SIGNATURE);
        assert_ne!(response_signature(&secret(), &"0".repeat(32), 200, br#"{"id":"x"}"#), RESPONSE_SIGNATURE);
        assert_ne!(response_signature(&secret(), NONCE_HEX, 200, br#"{"id":"y"}"#), RESPONSE_SIGNATURE);
    }

    #[test]
    fn any_change_to_what_was_signed_fails() {
        let signed = Signed::parse(&vector_headers()).unwrap();
        let s = secret();
        assert!(!signed.verify(&s, "GET", "/manage/v1/settings", BODY.as_bytes()), "method");
        assert!(!signed.verify(&s, "POST", "/manage/v1/restart", BODY.as_bytes()), "path");
        assert!(!signed.verify(&s, "POST", "/manage/v1/settings?x=1", BODY.as_bytes()), "query");
        assert!(!signed.verify(&s, "POST", "/manage/v1/settings", b"{}"), "body");
        let other = Secret::parse(&"x".repeat(32)).unwrap();
        assert!(!signed.verify(&other, "POST", "/manage/v1/settings", BODY.as_bytes()), "secret");
        let mut bob = vector_headers();
        bob.insert(ACTOR, HeaderValue::from_static("bob"));
        assert!(!Signed::parse(&bob).unwrap().verify(&s, "POST", "/manage/v1/settings", BODY.as_bytes()), "actor");
        let mut later = vector_headers();
        later.insert(TIMESTAMP, HeaderValue::from_static("1763640001"));
        assert!(!Signed::parse(&later).unwrap().verify(&s, "POST", "/manage/v1/settings", BODY.as_bytes()), "time");
        // The timestamp is signed as sent: a leading zero is another text.
        let mut padded = vector_headers();
        padded.insert(TIMESTAMP, HeaderValue::from_static("01763640000"));
        let padded = Signed::parse(&padded).unwrap();
        assert_eq!(padded.timestamp, 1_763_640_000);
        assert!(!padded.verify(&s, "POST", "/manage/v1/settings", BODY.as_bytes()));
        assert!(!verify(&s, "anything", "not hex"));
    }

    #[test]
    fn headers_of_the_wrong_form_or_given_twice_are_refused() {
        let good = [(TIMESTAMP, TS), (NONCE, NONCE_HEX), (ACTOR, "alice"), (SIGNATURE, REQUEST_SIGNATURE)];
        assert!(Signed::parse(&headers(&good)).is_some());
        let swap = |name: &'static str, value: &str| {
            let pairs: Vec<(&'static str, &str)> = good.iter().map(|&(n, v)| if n == name { (n, value) } else { (n, v) }).collect();
            Signed::parse(&headers(&pairs))
        };
        for bad in ["", "123456789012", "-1", "1e9", " 1", "１"] {
            assert!(swap(TIMESTAMP, bad).is_none(), "timestamp {bad:?}");
        }
        assert!(swap(TIMESTAMP, "0").is_some() && swap(TIMESTAMP, "99999999999").is_some());
        for bad in ["", "00112233445566778899AABBCCDDEEFF", "0011", &"0".repeat(33), "g0112233445566778899aabbccddeeff"] {
            assert!(swap(NONCE, bad).is_none(), "nonce {bad:?}");
        }
        for bad in ["", ".alice", "-a", "_a", "al ice", "al/ice", &"a".repeat(33), "ålice"] {
            assert!(swap(ACTOR, bad).is_none(), "actor {bad:?}");
        }
        for good in ["a", "Alice.B_c-d", &"a".repeat(32), "0"] {
            assert!(swap(ACTOR, good).is_some(), "actor {good:?}");
        }
        let hex = REQUEST_SIGNATURE.strip_prefix("v1=").unwrap();
        for bad in [hex, &format!("v2={hex}"), &format!("V1={hex}"), &format!("v1={}", hex.to_uppercase()), "v1=", &format!("v1={hex}0")] {
            assert!(swap(SIGNATURE, bad).is_none(), "signature {bad:?}");
        }
        for missing in [TIMESTAMP, NONCE, ACTOR, SIGNATURE] {
            let pairs: Vec<_> = good.iter().copied().filter(|(n, _)| *n != missing).collect();
            assert!(Signed::parse(&headers(&pairs)).is_none(), "without {missing}");
        }
        for twice in [TIMESTAMP, NONCE, ACTOR, SIGNATURE] {
            let mut pairs = good.to_vec();
            pairs.push(*good.iter().find(|(n, _)| *n == twice).unwrap());
            assert!(Signed::parse(&headers(&pairs)).is_none(), "{twice} twice");
        }
    }

    #[test]
    fn a_secret_is_checked_and_never_shown() {
        assert!(Secret::parse(&"a".repeat(31)).unwrap_err().contains("at least 32"));
        assert!(Secret::parse(&"a".repeat(257)).unwrap_err().contains("at most 256"));
        assert!(Secret::parse(&"a".repeat(32)).is_ok() && Secret::parse(&"a".repeat(256)).is_ok());
        for bad in [" ", "\"", "'", "\\", "\u{7f}", "é"] {
            let text = format!("{}{bad}", "a".repeat(40));
            let error = Secret::parse(&text).unwrap_err();
            assert!(!error.contains(&"a".repeat(40)), "{error}");
        }
        let s = secret();
        assert_eq!(format!("{s:?}"), "Secret(..)");
        assert!(s.is_in(&format!("x{SECRET}y")) && !s.is_in("nothing"));
    }
}
