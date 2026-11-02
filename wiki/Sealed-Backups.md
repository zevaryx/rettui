A backup can be sealed with a passphrase (see [Backups](Data-and-Storage#backups)
for making one). This page is how sealing works, what it protects against,
and how to open a sealed backup without rettui, to move your identity
somewhere else.

## In short

A sealed backup is the same `.tar.gz` as any other, encrypted as it's
written: the file holds a header, then the backup in encrypted chunks.
Nothing in it can be read without the passphrase, not even that it's gzip.

1. The passphrase is stretched with **Argon2id** into 32 bytes, which become
   an **X25519** key pair.
2. The backup is encrypted to the **public half**, through a new X25519 key
   of its own, **HKDF-SHA256** and **AES-256-GCM**, in 64 KiB chunks.
3. Opening it takes the passphrase again, which makes the private half again.

A key pair, rather than a key made straight from the passphrase, is what
lets backups be sealed with nobody there to type the passphrase: the
[Android app's scheduled backups](Android#identity-and-backups) keep only
the public half (a *backup key*), which seals backups but can't open them.
It's the same structure as the `age` tool's X25519 recipients.

## From passphrase to key pair

```text
seed   = Argon2id(passphrase, salt, 64 MiB, 3 passes, 1 lane, 32 bytes)
secret = the X25519 private key from seed
public = its public key
```

- **Argon2id** (version 0x13) is made for passphrases: each guess takes
  64 MiB of memory and three passes over it, which is slow on a computer
  and costly to speed up with graphics cards or custom chips. On a phone it
  takes about half a second, once for each backup or restore.
- **The salt** is 16 random bytes, new each time a key is made. The same
  passphrase makes a different key every time, so guesses worked out in
  advance are no use: each backup key has to be attacked on its own.
- The passphrase is used exactly as typed (UTF-8): spaces at either end
  count.
- The seed and the private key are wiped from memory once used.

## The file

```text
offset  size  what
0       22    "rettui sealed backup1\n"
22      4     Argon2 memory, KiB      (unsigned 32-bit, little-endian)
26      4     Argon2 passes
30      4     Argon2 lanes
34      16    salt
50      32    the backup key's public key
82      32    this backup's own public key (made for it, then thrown away)
114     …     the chunks
```

Each chunk is up to 64 KiB of the backup, encrypted, and a 16-byte tag;
every chunk but the last is 65,552 bytes. The first line tells a sealed
backup from a plain one, which starts with gzip's `1f 8b`.

## Sealing

```text
ephemeral = a new X25519 private key, for this backup alone
shared    = X25519(ephemeral, public)
key       = HKDF-SHA256(shared, salt = the whole 114-byte header, info = "rettui backup chunks"), 32 bytes
chunk i   = AES-256-GCM(key, nonce i, up to 64 KiB of the backup)
nonce i   = 3 zero bytes, i (8 bytes, big-endian), then 1 on the last chunk, 0 on the others
```

- **A key of its own for each backup:** two backups sealed with the same
  passphrase or backup key still have different chunk keys, so nonces never
  repeat between them.
- **The whole header goes into the key:** changing any byte of it (the
  costs, the salt, either public key) changes the key, and no chunk opens.
- **Numbered chunks** can't be reordered, repeated or dropped, and since the
  last says it's the last, the backup can't be cut short either. A full
  chunk is sealed only once more follows, so the last is never empty
  (unless the whole backup is).
- Only one chunk is held at a time, however large the backup, and it's
  wiped once written. The plain backup is never written to disk.

## Opening

1. Read the header and check the first line.
2. Check the costs it asks for: at most 1 GiB, 16 passes and 16 lanes. A
   file asking for more isn't one of rettui's, and could otherwise be made
   to stop rettui.
3. Stretch the passphrase with the salt and costs in the header. If the
   public key that makes isn't the one in the header, the passphrase is
   wrong, and rettui says so before opening anything.
4. Make the chunk key as above, and open the chunks one by one; each is
   checked by its tag. A changed, reordered, cut short or lengthened file
   fails with "the backup is damaged, or was changed".

Restoring reads a backup twice (once to check it, once to unpack it), and
stretches the passphrase once for both. Nothing is written into the data
directory until the whole backup has been read and checked.

## Backup keys

```text
rettui-backup-key-1:<base64 of: memory, passes, lanes (4 bytes each, little-endian), salt (16), public key (32)>
```

`rettui backup-key` turns a passphrase into one; `rettui backup --key KEY`
seals a backup with it, without asking for anything. A backup sealed with
a key is opened like any other, with the passphrase.

A backup key opens nothing: it's the public half, and its salt and costs
are in every backup's header anyway. Someone who has it can guess at the
passphrase, as they could with any one backup, at Argon2's cost a guess.

The Android app asks for a passphrase the first time scheduled backups are
turned on, runs `rettui backup-key` with it, and keeps only the key. The
passphrase isn't saved anywhere. **Change the passphrase…** makes a new
key; backups already made keep the passphrase they were sealed with.

## How each part of rettui asks

| Where | What it asks | How the passphrase travels |
|---|---|---|
| Terminal UI, `B` | After the file: the passphrase, then again, shown as dots. `Enter` alone: none. | It stays in rettui, and is wiped once the second is typed. It's stretched on the saving thread, not while drawing. |
| `rettui backup` | In a terminal, the same, and `Enter` alone for none. `--passphrase` needs one, `--no-passphrase` doesn't ask, `--key` seals with a backup key. Without a terminal, none, unless `RETTUI_BACKUP_PASSPHRASE` is set. | Typed unseen, or from the variable. |
| `rettui restore`, `rettui unseal` | A sealed backup's passphrase, in a terminal. | The same. |
| Web UI | **Download a backup** asks in a dialog; empty, the backup isn't sealed. | In the body of a request (`POST /api/backup`), never in an address. The sealed file waits in rettui's cache for 10 minutes under a random 128-bit name, which the browser downloads (`GET /api/backup/<id>`). The web UI's backups never have the identity. |
| Android app | *Save a full backup* asks first. Restoring asks only for a sealed backup. Scheduled backups: see above. | rettui runs inside the app, without a terminal: the app sets `RETTUI_BACKUP_PASSPHRASE` in its own process for that one command, then removes it. Other apps can't read it. |

A passphrase that's short (under 12 characters) or a PIN of digits alone
gets a warning everywhere it's typed. It's still yours to use.

## What it protects against

- Someone with a copy of the backup (in a cloud folder, on a lost USB
  stick, in a shared Downloads folder) reading your identity, contacts or
  messages, or restoring them to pose as you, unless they can guess the
  passphrase.
- A backup changed so that restoring it brings in someone else's settings
  or identity: any change fails, and nothing is restored.
- Restoring half a backup from a damaged or cut-short copy.
- A file made to ask for so much work that opening it stops rettui.

## What it doesn't

- **A passphrase that's easy to guess.** Whoever has the backup can try
  passphrases on their own computers, as fast as they can go, with nothing
  to stop them after a few tries. Argon2id makes each try slow, but a PIN
  of 4 to 6 digits is a million tries at most: under two weeks on one
  computer at a second each, much less on many. Use a few words.
- **A lost passphrase.** Nothing opens the backup without it.
- **A device someone else controls.** Whatever can run as you there can
  read rettui's data directory as it is. Sealing protects the copies that
  leave it.
- **What can be seen from outside:** the file's size and dates, and that
  it's a sealed rettui backup.
- Copies of the passphrase that rettui doesn't control (a browser's form
  fields, the app's text fields) are wiped when those programs choose.

## Opening a sealed backup without rettui

To move your identity (or everything) somewhere else, there are two ways
to open a sealed backup without starting rettui.

**`rettui unseal`** needs only the `rettui` program, not a running rettui or
a data directory:

```sh
rettui unseal rettui-backup-2026-11-02.tar.gz plain.tar.gz       # the plain backup
rettui unseal rettui-backup-2026-11-02.tar.gz --identity my-id   # only the identity
```

**`tools/unseal_backup.py`**, in rettui's repository, does the same with
Python 3.8 or later and two common packages, without rettui at all:

```sh
pip install cryptography argon2-cffi
python3 unseal_backup.py rettui-backup-2026-11-02.tar.gz plain.tar.gz
python3 unseal_backup.py rettui-backup-2026-11-02.tar.gz --identity my-id
```

Both ask for the passphrase (or take `RETTUI_BACKUP_PASSPHRASE`), never
write over a file, and make the new file readable only by you.

The plain backup is a `.tar.gz`: `tar xzf plain.tar.gz` unpacks it into
`rettui-backup/`, and `rettui restore plain.tar.gz` restores it. The
identity is a Reticulum identity file (64 bytes), the same as
`rettui identity export` saves:

- **Sideband, MeshChat:** bring it in as an identity file, their own way
  (it's the same kind of file they save).
- **NomadNet:** with NomadNet stopped, copy it over
  `~/.nomadnetwork/storage/identity` (keep the one there first).
- **Another rettui:** `rettui identity import my-id --name NAME`, or
  **Keep another identity from a file** in the Android app.

## Compatibility

- Plain backups are as they were, and restore as before. A passphrase given
  for one is ignored.
- rettui before v1.10.0 can't restore a sealed backup: it says it isn't a
  rettui backup. Open it with `rettui unseal` or the Python tool, and
  restore the plain backup.
- The format has its version in its first line (`backup1`) and in the key
  (`rettui-backup-key-1:`), and each file carries its own Argon2 costs, so
  later versions can raise them and still open older backups.

## Libraries

Pure Rust, from the RustCrypto and dalek projects (MIT or Apache-2.0):
`argon2`, `x25519-dalek`, `aes-gcm`, `hkdf` and `sha2`, with `zeroize` for
wiping secrets, and `rpassword` for typing a passphrase unseen in a
terminal. The Python tool uses `cryptography` and `argon2-cffi`.
