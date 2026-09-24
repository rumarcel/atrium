# ADR-019 — The armed pairing secret is stored sealed under `secrets.key`, never as plaintext and never as a bare digest

**Status:** accepted (M1E, 2026-09-24). Supersedes only ADR-003 §3's
"never persists it in plaintext — only a hash, for attempt counting" and the
matching "stored only as `SHA-256`" lines of plan §4.3 and §6.3
**Date:** 2026-09-24
**Related:** ADR-003 (§3, §4, §7), ADR-012, ADR-016, ADR-017
**Supersedes:** the storage sentences named above, and nothing else in ADR-003

## Context

ADR-003 §4 derives the pairing key from the secret itself:

```
K = HKDF-SHA256(ikm = secret16, salt = serverNonce, info = "atrium-pair-v1" ‖ serverId)
proofC = HMAC-SHA256(K, "atrium-pair-v1:client" ‖ T)
```

To check `proofC`, Core has to compute `K`, and to compute `K` it needs the 16
secret bytes. ADR-003 §3 also said Core "never persists it in plaintext —
only a hash, for attempt counting", and plan §4.3 gave `pairing_state` a
`secret_digest` column. A digest cannot produce `K`. Taken together the two
statements cannot be implemented: the secret is armed by `atriumctl pair` —
a different process, possibly before Core starts, and in any case surviving a
Core restart inside its 15-minute window — so it has to be stored in a form
Core can recover.

M1E stopped on the contradiction and the owner decided (2026-09-24): store the
secret recoverably, but only as authenticated ciphertext from a standard AEAD
in a mature crate, under `secrets.key`, bound to its context; do not invent a
wrapping construction; keep the digest only if it has an independent purpose.

## Decision

**1. Construction.** XChaCha20-Poly1305 (RustCrypto `chacha20poly1305`
0.10), with:

```
key   = HKDF-SHA256(ikm = secrets.key, salt = none,
                    info = "atrium-pairing-secret-v1/key", L = 32)
nonce = 24 bytes from the OS CSPRNG, fresh for every arming
aad   = "atrium-pairing-secret-v1" ‖ serverId(16) ‖ armingId(16)
        ‖ SHA-256(binding profile id)(32)
        ‖ armedAt (i64 big-endian Unix seconds) ‖ expiresAt (same)
```

`armingId` is 16 random bytes naming this arming. The sealed value is the 16
secret bytes plus the 16-byte tag. XChaCha20-Poly1305 was preferred to
AES-256-GCM because its 192-bit nonce makes a random nonce safe without a
counter to persist, and it is constant-time in software on every target
(no dependence on AES-NI).

**2. Storage.** `pairing_state` (migration 2) holds `arming_id`, `profile`,
`secret_nonce` (24 bytes), `secret_ciphertext` (32 bytes), `armed_at` and
`expires_at`. A `CHECK` admits exactly two shapes: armed, with every field
present and the only implemented profile; or not armed, with none of them.
There is no plaintext column and no digest column: `secret_digest` from
migration 1 is dropped because it has no remaining purpose (attempt counting
is `failures`, and single use is the deletion below). Any row migration 1
recorded as armed comes across disarmed.

**3. Key.** `secrets.key` stays exactly as ADR-017 left it: 32 bytes,
`root:atrium 0640` in `/etc/atrium`, created at installation, never in the
database, never in a backup of the database. Core reads it at startup (a
failure is recovery mode); `atriumctl pair` and `rotate-identity` read it
after dropping to the service user.

**4. Lifetime.** The ciphertext is deleted in the same transaction as the
event that ends it:

| Event | Where |
| --- | --- |
| consumed by a successful `pair/complete` | the transaction that inserts the device and claims the server |
| expired | the next pairing request, or Core's 30-second sweep, whichever is first |
| ~~fifth failed proof~~ | *removed by [ADR-020](0020-no-failed-proof-lock.md): failures never end the secret* |
| replaced by a new `atriumctl pair` | the arming transaction (one row, overwritten) |
| identity key changed | `reconcile_identity`'s revocation transaction |

**5. Use.** Core opens the ciphertext only inside `pair/complete`, only for
an attempt whose `armingId` is the armed one, and holds the plaintext as a
zeroizing value for the length of the proof computation. An opening failure —
a different `secrets.key`, a modified nonce, ciphertext or context field —
refuses the attempt with the generic `pairing.rejected`, is logged as
`pairing_secret_unsealable`, does not count against the arming, and **does
not generate, re-arm or repair anything**: the operator runs `atriumctl pair`.

## Threat model

- **Database-only compromise** (a copied `atrium.db`, a backup, a stolen
  disk image without `/etc/atrium`): reveals ciphertext bound to one server,
  one arming and one 15-minute window. Without `secrets.key` it yields
  nothing, and after the window it is useless even with it.
- **`secrets.key` without the database:** nothing to open.
- **A compromised running Core** can recover an armed secret. That is inside
  Core's existing trust boundary — Core must verify `proofC`, so it must be
  able to compute `K` — and such an attacker can already mint device rows
  directly. Nothing is claimed beyond it.
- **Tampering with the row** (extending `expires_at`, moving a ciphertext to
  another arming, relabelling its profile) breaks authentication and is
  refused.

## Consequences

- ADR-003 §3's other rules are unchanged: 16 bytes from the CSPRNG, 26
  Crockford symbols, canonical decoding, constant-time comparison of decoded
  bytes, single use, 15-minute life, never logged, never returned, never in
  diagnostics.
- Tests prove each property: no plaintext in the database or a backup, no
  opening under another `secrets.key`, tampered ciphertext or context
  refused, nothing recoverable after consumption, expiry, lock or
  replacement, completion still possible after a Core restart inside the
  window, and no crash point that leaves a consumed secret usable
  (`atrium-core` `pairing::seal`, `pairing::tests`, `db::pairing::tests`,
  `http::pairing_tests`; `atriumctl` `console` and `privileged`).
- One more dependency family member (`chacha20poly1305`, RustCrypto, the same
  maintainers as `hkdf`, `hmac` and `sha2`).

## Alternatives considered

- **Digest only (ADR-003 as written).** Cannot verify `proofC`. Rejected as
  unimplementable.
- **Plaintext in the database.** A backup or a copied file would carry a
  live secret. Rejected.
- **A hand-rolled wrap (for example HMAC-SHA256 keystream XOR).** Rejected by
  the owner: no new construction when a reviewed AEAD exists.
- **Keeping the secret only in Core's memory** (Core generates and shows it).
  Would move arming into the network service and lose it on every restart;
  ADR-003 keeps arming a console action. Rejected.
- **AES-256-GCM.** Sound, but a 96-bit random nonce is the weaker default and
  software AES is not constant-time everywhere; nothing in the repository
  favoured it.
