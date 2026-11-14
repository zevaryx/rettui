#!/usr/bin/env python3
"""Open a rettui backup sealed with a passphrase, without rettui.

Writes the plain backup (a .tar.gz, as `rettui backup` makes without a
passphrase), or with --identity only the identity in it: a Reticulum
identity file, as Sideband, NomadNet and MeshChat use. Nothing else is
needed but Python 3.8 or later and two packages:

    pip install cryptography argon2-cffi
    python3 unseal_backup.py rettui-backup-2026-11-02.tar.gz plain.tar.gz
    python3 unseal_backup.py rettui-backup-2026-11-02.tar.gz --identity my-identity

It asks for the passphrase, or takes it from RETTUI_BACKUP_PASSPHRASE. The
format is described on the wiki's Sealed Backups page, and in rettui's
src/backup/sealed.rs, which this follows.
"""

import argparse
import getpass
import io
import os
import struct
import sys
import tarfile

try:
    from argon2.low_level import Type, hash_secret_raw
    from cryptography.exceptions import InvalidTag
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    from cryptography.hazmat.primitives.kdf.hkdf import HKDF
    from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
except ImportError:
    sys.exit("This needs two Python packages: pip install cryptography argon2-cffi")

MAGIC = b"rettui sealed backup1\n"
SALT = 16
HEADER = len(MAGIC) + 12 + SALT + 32 + 32
CHUNK = 64 * 1024
TAG = 16
INFO = b"rettui backup chunks"
# The most a backup may ask for, as rettui allows: more is no backup of rettui's.
MOST_MEMORY_KIB, MOST_PASSES, MOST_LANES = 1024 * 1024, 16, 16
# Where the identity is in a backup.
IDENTITY = "rettui-backup/identity"


class Unsealing(Exception):
    pass


def raw(public):
    return public.public_bytes(Encoding.Raw, PublicFormat.Raw)


def unseal(sealed, passphrase, out):
    """Write the backup sealed in the file `sealed` to `out`, opened with `passphrase`."""
    header = sealed.read(HEADER)
    if len(header) < HEADER or not header.startswith(MAGIC):
        raise Unsealing("it isn't a rettui backup sealed with a passphrase")
    memory, passes, lanes = struct.unpack_from("<III", header, len(MAGIC))
    if memory > MOST_MEMORY_KIB or passes > MOST_PASSES or lanes > MOST_LANES:
        raise Unsealing("it asks for more work to open than any rettui backup does")
    at = len(MAGIC) + 12
    salt, public, ephemeral = header[at:at + SALT], header[at + SALT:at + SALT + 32], header[at + SALT + 32:]
    # The passphrase, stretched as rettui does, is the private key.
    seed = hash_secret_raw(passphrase.encode("utf-8"), salt, time_cost=passes, memory_cost=memory, parallelism=lanes,
                           hash_len=32, type=Type.ID, version=19)
    secret = X25519PrivateKey.from_private_bytes(seed)
    if raw(secret.public_key()) != public:
        raise Unsealing("that isn't its passphrase")
    shared = secret.exchange(X25519PublicKey.from_public_bytes(ephemeral))
    key = HKDF(algorithm=hashes.SHA256(), length=32, salt=header, info=INFO).derive(shared)
    cipher = AESGCM(key)
    count = 0
    chunk = sealed.read(CHUNK + TAG)
    while True:
        # A short chunk is the last; a full one is, if nothing follows.
        following = sealed.read(CHUNK + TAG) if len(chunk) == CHUNK + TAG else b""
        last = not following
        nonce = bytes(3) + count.to_bytes(8, "big") + bytes([1 if last else 0])
        try:
            out.write(cipher.decrypt(nonce, chunk, None))
        except InvalidTag:
            raise Unsealing("the backup is damaged, or was changed") from None
        if last:
            return
        chunk, count = following, count + 1


def main():
    parser = argparse.ArgumentParser(description="Open a rettui backup sealed with a passphrase, without rettui.")
    parser.add_argument("sealed", help="the sealed backup")
    parser.add_argument("out", nargs="?", help="where to write the plain backup (a .tar.gz)")
    parser.add_argument("--identity", metavar="FILE", help="write only the identity in it, to FILE")
    args = parser.parse_args()
    if bool(args.out) == bool(args.identity):
        parser.error("give where to write the plain backup, or --identity FILE (one of them)")
    target = args.out or args.identity
    if os.path.exists(target):
        sys.exit(f"{target} exists already: pick another name")
    passphrase = os.environ.get("RETTUI_BACKUP_PASSPHRASE") or getpass.getpass("Its passphrase: ")
    plain = io.BytesIO()
    try:
        with open(args.sealed, "rb") as sealed:
            unseal(sealed, passphrase, plain)
    except (OSError, Unsealing) as e:
        sys.exit(f"Couldn't open {args.sealed}: {e}")
    data = plain.getvalue()
    if args.identity:
        try:
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
                member = tar.getmember(IDENTITY)
                data = tar.extractfile(member).read()
        except (KeyError, tarfile.TarError):
            sys.exit(f"{args.sealed} has no identity in it (backed up with --without-identity, or from the web UI)")
    # Only you may read it: it may hold the identity.
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as out:
        out.write(data)
    if args.identity:
        print(f"Saved the identity to {target}. Keep it private: whoever has it can pose as you.")
    else:
        print(f"Saved the plain backup to {target}. It may hold your identity: keep it private.")


if __name__ == "__main__":
    main()
