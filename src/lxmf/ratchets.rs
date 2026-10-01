//! Ratchets for this client's LXMF address. Like Python LXMF, which gives
//! every delivery address one, the address keeps a ring of keys that rotate
//! and announces the newest: senders encrypt to it, so a message can't be
//! read later from the identity's key alone (forward secrecy). The network
//! actor's destination owns the ring, and rotates it as it announces; the
//! other places that decrypt messages for this client (a propagation node
//! sync, a paper message, the hosted propagation node) read it.

use std::path::{Path, PathBuf};

use rns_identity::identity::{Identity, IdentityError};
use rns_identity::ratchet::RatchetRing;

use crate::net::Hash;

/// The ring file for an LXMF address, in `dir`, named as Python LXMF names
/// it.
pub fn ring_path(dir: &Path, lxmf_hash: &Hash) -> PathBuf {
    dir.join(format!("{}.ratchets", hex::encode(lxmf_hash)))
}

/// Decrypt a message sent to this client: with one of its ratchets, or
/// with the identity's key, from a sender that hadn't heard one.
pub fn decrypt(identity: &Identity, ring: &Path, ciphertext: &[u8]) -> Result<Vec<u8>, IdentityError> {
    let ring = RatchetRing::load_verified(ring, identity).ok().map(|loaded| loaded.into_ring());
    let keys: Vec<&[u8; 32]> = ring.iter().flat_map(|ring| ring.private_keys()).collect();
    identity.decrypt(ciphertext, Some(&keys), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rns_identity::ratchet::PersistentRatchetRing;

    #[test]
    fn a_message_to_a_ratchet_or_to_the_identity_is_read() {
        let dir = std::env::temp_dir().join(format!("rettui-ratchets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let identity = Identity::new();
        let ring = ring_path(&dir, &[7; 16]);
        assert_eq!(ring.file_name().unwrap(), "07070707070707070707070707070707.ratchets");
        // No ring yet: the identity's key.
        let public = Identity::from_public_key(&identity.get_public_key()).unwrap();
        let plain = public.encrypt(b"before", None).unwrap();
        assert_eq!(decrypt(&identity, &ring, &plain).unwrap(), b"before");
        // To the ratchet announced: read with the ring, and not without it.
        let announced = PersistentRatchetRing::open(&ring, &identity).unwrap().ensure_current(&identity).unwrap();
        let sealed = public.encrypt(b"after", Some(&announced)).unwrap();
        assert_eq!(decrypt(&identity, &ring, &sealed).unwrap(), b"after");
        assert!(identity.decrypt(&sealed, None, false).is_err());
        assert_eq!(decrypt(&identity, &ring, &plain).unwrap(), b"before");
        // Another identity's ring isn't trusted.
        assert!(decrypt(&Identity::new(), &ring, &sealed).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
