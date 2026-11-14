//! Backups sealed with a passphrase: only someone who knows it can open
//! one, and so restore it.
//!
//! The passphrase is stretched with Argon2id (slow, and costly in memory,
//! so guessing is too) into an X25519 key pair. A backup is sealed to the
//! public half, the [`BackupKey`], which needs no passphrase: rettui can
//! seal backups on a schedule knowing only that. Opening one takes the
//! passphrase again.
//!
//! A sealed backup is the header, then the backup in AES-256-GCM chunks:
//!
//! ```text
//! MAGIC | m_kib t p (u32 LE each) | salt (16) | public key (32) | ephemeral public key (32)
//! chunk* : 64 KiB of the backup + 16-byte tag; nonce = 11-byte big-endian count, 1 on the last
//! ```
//!
//! The chunks' key comes from X25519 between the ephemeral key and the
//! public key, through HKDF-SHA256 salted with the whole header, so nothing
//! in it can be changed unnoticed. A chunk can't be dropped, moved or
//! repeated, and the end can't be cut off: the last says it's the last.

use std::io::{Read, Write};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use zeroize::Zeroizing;

/// The start of every sealed backup.
pub const MAGIC: &[u8; 22] = b"rettui sealed backup1\n";
/// Argon2id's cost for new keys: 64 MiB, 3 passes, one lane (about half a
/// second on a phone).
const MEMORY_KIB: u32 = 64 * 1024;
const PASSES: u32 = 3;
const LANES: u32 = 1;
/// The most a backup may ask for (it's read from the file, which anyone
/// may have made): more would be a way to stop rettui, not a backup.
const MOST_MEMORY_KIB: u32 = 1024 * 1024;
const MOST_PASSES: u32 = 16;
const MOST_LANES: u32 = 16;
const SALT: usize = 16;
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
const HEADER: usize = MAGIC.len() + 12 + SALT + 32 + 32;
/// How a backup key is written: this, then base64 of its cost, salt and public key.
const KEY_PREFIX: &str = "rettui-backup-key-1:";
/// What the chunks' key is for (HKDF's info).
const INFO: &[u8] = b"rettui backup chunks";

/// Fewer characters than this, and a passphrase is called short.
pub const SHORT: usize = 12;

/// Why `passphrase` is easy to guess, if it is: short, or digits alone
/// (a PIN). Only a warning: it's still theirs to use. (The web UI and the
/// Android app say the same, by the same rule.)
pub fn weakness(passphrase: &str) -> Option<&'static str> {
    let length = passphrase.chars().count();
    if length == 0 {
        None
    } else if length < 20 && passphrase.chars().all(|c| c.is_ascii_digit()) {
        Some(
            "A PIN of digits alone is easy to guess: anyone with the backup can try every one, as fast as their computer goes, with nothing to stop them. A few words are much safer.",
        )
    } else if length < SHORT {
        Some(
            "That passphrase is short: anyone with the backup can try passphrases as fast as their computer goes, with nothing to stop them. A few words (12 characters or more) are much safer.",
        )
    } else {
        None
    }
}

/// How a passphrase is stretched: Argon2id's cost and salt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stretch {
    memory_kib: u32,
    passes: u32,
    lanes: u32,
    salt: [u8; SALT],
}

impl Stretch {
    fn new() -> Self {
        let mut salt = [0u8; SALT];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut salt);
        Stretch { memory_kib: MEMORY_KIB, passes: PASSES, lanes: LANES, salt }
    }

    fn to_bytes(self) -> [u8; 12 + SALT] {
        let mut out = [0u8; 12 + SALT];
        out[0..4].copy_from_slice(&self.memory_kib.to_le_bytes());
        out[4..8].copy_from_slice(&self.passes.to_le_bytes());
        out[8..12].copy_from_slice(&self.lanes.to_le_bytes());
        out[12..].copy_from_slice(&self.salt);
        out
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
        let stretch =
            Stretch { memory_kib: word(0), passes: word(4), lanes: word(8), salt: bytes[12..12 + SALT].try_into().unwrap_or_default() };
        if stretch.memory_kib > MOST_MEMORY_KIB || stretch.passes > MOST_PASSES || stretch.lanes > MOST_LANES {
            return Err("it asks for more work to open than any rettui backup does".into());
        }
        Ok(stretch)
    }

    /// The key pair `passphrase` stretches to.
    fn secret(self, passphrase: &str) -> Result<x25519_dalek::StaticSecret, String> {
        let params =
            argon2::Params::new(self.memory_kib, self.passes, self.lanes, Some(32)).map_err(|e| format!("its passphrase settings: {e}"))?;
        let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut seed = Zeroizing::new([0u8; 32]);
        argon
            .hash_password_into(passphrase.as_bytes(), &self.salt, seed.as_mut())
            .map_err(|e| format!("Couldn't use the passphrase: {e}"))?;
        Ok(x25519_dalek::StaticSecret::from(*seed))
    }
}

/// What seals backups for a passphrase, without the passphrase: safe to
/// keep where rettui makes backups on its own (the Android app's schedule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupKey {
    stretch: Stretch,
    public: [u8; 32],
}

impl BackupKey {
    /// The key for `passphrase`, with a new salt (slow, on purpose).
    pub fn from_passphrase(passphrase: &str) -> Result<Self, String> {
        let stretch = Stretch::new();
        let public = x25519_dalek::PublicKey::from(&stretch.secret(passphrase)?).to_bytes();
        Ok(BackupKey { stretch, public })
    }

    /// As written down: `rettui-backup-key-1:<base64>`.
    pub fn encode(&self) -> String {
        let mut bytes = self.stretch.to_bytes().to_vec();
        bytes.extend_from_slice(&self.public);
        format!("{KEY_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let bad = || "That isn't a rettui backup key (rettui-backup-key-1:…)".to_string();
        let encoded = text.trim().strip_prefix(KEY_PREFIX).ok_or_else(bad)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| bad())?;
        if bytes.len() != 12 + SALT + 32 {
            return Err(bad());
        }
        let stretch = Stretch::from_bytes(&bytes[..12 + SALT])?;
        Ok(BackupKey { stretch, public: bytes[12 + SALT..].try_into().map_err(|_| bad())? })
    }
}

/// The chunks' cipher, from the X25519 secret both ends share and the header.
fn cipher(shared: &[u8; 32], header: &[u8]) -> Aes256Gcm {
    let mut key = Zeroizing::new([0u8; 32]);
    // 32 bytes is always within what HKDF-SHA256 gives.
    let _ = hkdf::Hkdf::<sha2::Sha256>::new(Some(header), shared).expand(INFO, key.as_mut());
    Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(key.as_ref()))
}

fn nonce(count: u64, last: bool) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[3..11].copy_from_slice(&count.to_be_bytes());
    nonce[11] = u8::from(last);
    nonce
}

/// Writes a backup sealed to a [`BackupKey`] into `out`; [`Sealer::finish`]
/// writes its last chunk.
pub struct Sealer<W: Write> {
    out: W,
    cipher: Aes256Gcm,
    buffer: Zeroizing<Vec<u8>>,
    count: u64,
}

impl<W: Write> Sealer<W> {
    pub fn new(mut out: W, key: &BackupKey) -> std::io::Result<Self> {
        let mut ephemeral = Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, ephemeral.as_mut());
        let ephemeral = x25519_dalek::StaticSecret::from(*ephemeral);
        let mut header = Vec::with_capacity(HEADER);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&key.stretch.to_bytes());
        header.extend_from_slice(&key.public);
        header.extend_from_slice(x25519_dalek::PublicKey::from(&ephemeral).as_bytes());
        let shared = ephemeral.diffie_hellman(&x25519_dalek::PublicKey::from(key.public));
        out.write_all(&header)?;
        Ok(Sealer { out, cipher: cipher(shared.as_bytes(), &header), buffer: Zeroizing::new(Vec::with_capacity(CHUNK)), count: 0 })
    }

    fn seal(&mut self, last: bool) -> std::io::Result<()> {
        let nonce = nonce(self.count, last);
        let sealed = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: &self.buffer, aad: &[] })
            .map_err(|_| std::io::Error::other("couldn't seal the backup"))?;
        self.out.write_all(&sealed)?;
        self.buffer.clear();
        self.count += 1;
        Ok(())
    }

    /// Seal what's left, as the last chunk; the writer underneath.
    pub fn finish(mut self) -> std::io::Result<W> {
        self.seal(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Sealer<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        // A full chunk goes only once more follows, so the last is never empty
        // (unless the whole backup is).
        if self.buffer.len() == CHUNK && !data.is_empty() {
            self.seal(false)?;
        }
        let taken = data.len().min(CHUNK - self.buffer.len());
        self.buffer.extend_from_slice(&data[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Whether `start` (a file's first bytes) is a sealed backup's.
pub fn is_sealed(start: &[u8]) -> bool {
    start.starts_with(MAGIC)
}

/// What opens one sealed backup, made from its header and the passphrase
/// (the slow part, so done once for reading it twice).
pub struct Opener {
    header: [u8; HEADER],
    secret: x25519_dalek::StaticSecret,
}

/// Why a sealed backup couldn't be opened: it isn't one, or the passphrase is wrong.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    NotSealed,
    WrongPassphrase,
    Bad(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotSealed => write!(f, "it isn't sealed with a passphrase"),
            OpenError::WrongPassphrase => write!(f, "that isn't its passphrase"),
            OpenError::Bad(why) => write!(f, "{why}"),
        }
    }
}

impl Opener {
    /// Read the header from `input` and check `passphrase` against it.
    pub fn new(mut input: impl Read, passphrase: &str) -> Result<Self, OpenError> {
        let mut header = [0u8; HEADER];
        input.read_exact(&mut header).map_err(|_| OpenError::NotSealed)?;
        if !is_sealed(&header) {
            return Err(OpenError::NotSealed);
        }
        let stretch = Stretch::from_bytes(&header[MAGIC.len()..MAGIC.len() + 12 + SALT]).map_err(OpenError::Bad)?;
        let secret = stretch.secret(passphrase).map_err(OpenError::Bad)?;
        let public = &header[MAGIC.len() + 12 + SALT..MAGIC.len() + 12 + SALT + 32];
        if x25519_dalek::PublicKey::from(&secret).as_bytes() != public {
            return Err(OpenError::WrongPassphrase);
        }
        Ok(Opener { header, secret })
    }

    /// Read the backup sealed in `input` (from its start, header and all).
    pub fn open<R: Read>(&self, mut input: R) -> std::io::Result<Unsealer<R>> {
        let mut header = [0u8; HEADER];
        input.read_exact(&mut header)?;
        if header != self.header {
            return Err(std::io::Error::other("the backup changed while it was read"));
        }
        let ephemeral: [u8; 32] = header[HEADER - 32..].try_into().map_err(std::io::Error::other)?;
        let shared = self.secret.diffie_hellman(&x25519_dalek::PublicKey::from(ephemeral));
        Ok(Unsealer {
            input,
            cipher: cipher(shared.as_bytes(), &header),
            count: 0,
            chunk: Zeroizing::new(Vec::new()),
            at: 0,
            next: Vec::new(),
            done: false,
        })
    }
}

/// Reads a sealed backup's contents, checking each chunk.
pub struct Unsealer<R: Read> {
    input: R,
    cipher: Aes256Gcm,
    count: u64,
    /// The chunk being read, and how far.
    chunk: Zeroizing<Vec<u8>>,
    at: usize,
    /// The next chunk's sealed bytes, read ahead to know whether this is the last.
    next: Vec<u8>,
    done: bool,
}

impl<R: Read> Unsealer<R> {
    /// Up to one sealed chunk from the input.
    fn read_sealed(&mut self) -> std::io::Result<Vec<u8>> {
        let mut sealed = Vec::with_capacity(CHUNK + TAG);
        (&mut self.input).take((CHUNK + TAG) as u64).read_to_end(&mut sealed)?;
        Ok(sealed)
    }

    fn next_chunk(&mut self) -> std::io::Result<()> {
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "the backup is damaged, or was changed");
        let sealed = if self.count == 0 { self.read_sealed()? } else { std::mem::take(&mut self.next) };
        // A short chunk is the last; a full one is, if nothing follows.
        let last = if sealed.len() < CHUNK + TAG {
            true
        } else {
            self.next = self.read_sealed()?;
            self.next.is_empty()
        };
        let opened =
            self.cipher.decrypt(Nonce::from_slice(&nonce(self.count, last)), Payload { msg: &sealed, aad: &[] }).map_err(|_| bad())?;
        self.chunk = Zeroizing::new(opened);
        self.at = 0;
        self.count += 1;
        self.done = last;
        Ok(())
    }
}

impl<R: Read> Read for Unsealer<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.at == self.chunk.len() {
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let n = out.len().min(self.chunk.len() - self.at);
        out[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A key for `passphrase` that's quick to make (and so to guess): for tests only.
#[cfg(test)]
pub fn quick_key(passphrase: &str) -> BackupKey {
    let stretch = Stretch { memory_kib: 64, passes: 1, lanes: 1, salt: [9; SALT] };
    BackupKey { stretch, public: x25519_dalek::PublicKey::from(&stretch.secret(passphrase).unwrap()).to_bytes() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key(passphrase: &str) -> BackupKey {
        quick_key(passphrase)
    }

    fn seal(key: &BackupKey, data: &[u8]) -> Vec<u8> {
        let mut sealer = Sealer::new(Vec::new(), key).unwrap();
        sealer.write_all(data).unwrap();
        sealer.finish().unwrap()
    }

    fn open(sealed: &[u8], passphrase: &str) -> Result<Vec<u8>, String> {
        let opener = Opener::new(sealed, passphrase).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        opener.open(sealed).map_err(|e| e.to_string())?.read_to_end(&mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }

    #[test]
    fn opened_with_the_passphrase_whatever_its_length() {
        let key = test_key("correct horse");
        for size in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK, 3 * CHUNK + 17] {
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let sealed = seal(&key, &data);
            assert!(is_sealed(&sealed));
            assert_eq!(open(&sealed, "correct horse").unwrap(), data, "{size} bytes");
        }
    }

    #[test]
    fn not_opened_with_another_passphrase() {
        let sealed = seal(&test_key("correct horse"), b"secret");
        assert_eq!(Opener::new(&sealed[..], "wrong horse").err(), Some(OpenError::WrongPassphrase));
        assert_eq!(Opener::new(&b"\x1f\x8b plain gzip"[..], "x").err(), Some(OpenError::NotSealed));
    }

    #[test]
    fn changed_cut_short_or_reordered_isnt_opened() {
        let key = test_key("pass");
        let data: Vec<u8> = (0..3 * CHUNK).map(|i| (i % 7) as u8).collect();
        let sealed = seal(&key, &data);
        // A byte changed in a chunk.
        let mut changed = sealed.clone();
        changed[HEADER + 100] ^= 1;
        assert!(open(&changed, "pass").is_err());
        // The header changed (its ephemeral key).
        let mut changed = sealed.clone();
        changed[HEADER - 1] ^= 1;
        assert!(open(&changed, "pass").is_err());
        // The last chunk cut off: the one before isn't marked last.
        let cut = &sealed[..HEADER + 2 * (CHUNK + TAG)];
        assert!(open(cut, "pass").is_err());
        // Two chunks swapped.
        let mut swapped = sealed[..HEADER].to_vec();
        swapped.extend_from_slice(&sealed[HEADER + CHUNK + TAG..HEADER + 2 * (CHUNK + TAG)]);
        swapped.extend_from_slice(&sealed[HEADER..HEADER + CHUNK + TAG]);
        swapped.extend_from_slice(&sealed[HEADER + 2 * (CHUNK + TAG)..]);
        assert!(open(&swapped, "pass").is_err());
    }

    #[test]
    fn a_key_is_written_down_and_read_back() {
        let key = test_key("pass");
        let text = key.encode();
        assert!(text.starts_with(KEY_PREFIX));
        assert_eq!(BackupKey::decode(&text).unwrap(), key);
        assert!(BackupKey::decode("rettui-backup-key-1:AAAA").is_err());
        assert!(BackupKey::decode("something else").is_err());
    }

    #[test]
    fn a_backup_asking_too_much_work_isnt_tried() {
        let mut stretch = Stretch { memory_kib: MOST_MEMORY_KIB + 1, passes: 1, lanes: 1, salt: [0; SALT] }.to_bytes().to_vec();
        assert!(Stretch::from_bytes(&stretch).is_err());
        stretch[0..4].copy_from_slice(&64u32.to_le_bytes());
        assert!(Stretch::from_bytes(&stretch).is_ok());
    }

    #[test]
    fn short_passphrases_and_pins_are_called_weak() {
        assert!(weakness("").is_none());
        assert!(weakness("123456").unwrap().contains("PIN"));
        assert!(weakness("1234567890123456").unwrap().contains("PIN"));
        assert!(weakness("hunter2").unwrap().contains("short"));
        assert!(weakness("correct horse battery").is_none());
        assert!(weakness("12345678901234567890").is_none());
    }

    #[test]
    fn a_new_key_has_its_own_salt() {
        let a = BackupKey::from_passphrase("same").unwrap();
        let b = BackupKey::from_passphrase("same").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.stretch.memory_kib, MEMORY_KIB);
        let sealed = seal(&a, b"data");
        assert_eq!(open(&sealed, "same").unwrap(), b"data");
    }
}
