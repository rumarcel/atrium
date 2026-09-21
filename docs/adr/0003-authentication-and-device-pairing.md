# ADR-003 — Server identity, SPKI pinning and code-based pairing

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-006, ADR-009
**Assumptions:** A-06, A-07, A-08, A-24, A-30

The first revision of this ADR described the pairing exchange in prose. A security
review found two things wrong with it: the channel binding used a hand-rolled input
rather than the standard TLS channel binding, and the entropy of the pairing secret
was never specified, which silently made the whole construction depend on an
unstated assumption.

A second review found that the fix for the second problem was itself wrong — it
said "32 bytes rendered as 26 Crockford base32 characters", and 32 bytes is 256
bits, which needs 52 symbols. The generator is now pinned at **16 bytes / 128 bits
/ 26 symbols**, with the encoding, normalization and canonicality rules written out
in §3 rather than left to whoever implements it. That review also corrected the
claim that TLS 1.3 was mandatory *because* exporters need it; see §4.

The exchange is now specified precisely enough to implement and to attack.

## Context

The prototype enrolls a server by typing its private IPv4 address, pasting its
certificate PEM, and storing a bearer token in Windows Credential Manager under a
key that contains the address. Trust is therefore bound to an IP. A DHCP lease
change breaks it. A multi-homed server has no single answer. A second client
repeats the whole manual procedure.

The product needs a normal person to pair a client in under a minute, on a network
that may contain a hostile device (T1), against a server whose address may change
tomorrow — with no account, no cloud, and no certificate authority.

## Decision

### 1. Identity

At first start Core generates `server_id`: 128 random bits, stored in
`/etc/atrium/identity.json` (`0600`), stable for the life of the installation.
Identity is not the address, not the hostname, not the certificate.

Core also generates a TLS key pair and a self-signed leaf certificate whose SANs
cover its current addresses, `atrium-<short-id>.local` and `localhost`. **The
certificate is reissued automatically when addresses change; the key pair is
not.** Clients pin `SHA-256(SubjectPublicKeyInfo)`, so re-issuance is invisible to
a paired client.

### 2. Discovery

Core advertises `_atrium._tcp.local` with TXT records `id`, `name`, `version` and
a short SPKI prefix for display. Discovery is **advisory only** — an mDNS record is
trivially spoofed, so nothing is trusted because it was discovered. Manual address
entry is a first-class, equally supported path (A-08).

### 3. The pairing secret

**This is a high-entropy pairing secret, not a short human PIN.** It is generated
by the machine, copied or scanned, and thrown away. Nothing in the design assumes
anyone memorises it, and the interface never presents it as if they should.

**Generation and encoding — exactly, because an earlier draft of this ADR got the
arithmetic wrong and said "32 bytes rendered as 26 characters", which is not a
thing that can happen:**

| Property | Value |
| --- | --- |
| Entropy | **128 bits**, a requirement (see §5), not a default |
| Generated as | **exactly 16 bytes** from the operating system CSPRNG |
| Encoded as | **exactly 26 Crockford Base32 symbols** |
| Why 26 | 16 bytes = 128 bits; ⌈128 ÷ 5⌉ = 26 symbols, carrying 130 bit positions |
| Padding | The final symbol carries 3 significant bits; its 2 low bits are padding and **must be zero** |
| Displayed as | `XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XX` — six groups of four and a final pair |

The hyphens are **presentation only**. They carry no entropy, they are not part of
the secret, and removing or adding them cannot change the value that is decoded.
The secret is 128 bits whether it is shown grouped, ungrouped, or as a QR code.

**Decoding is normalized before anything else happens, in this exact order:**

1. Remove every ASCII and Unicode whitespace character, and every `-`.
2. Uppercase using an **ASCII-only** mapping. Locale-sensitive case conversion is
   forbidden: in a Turkish locale `"i".toUpperCase()` is `"İ"`, which would make a
   correct secret un-decodable on the owner's own machine. This is a real bug in a
   real locale, not a hypothetical.
3. Apply Crockford's confusable mapping: `O` → `0`, `I` → `1`, `L` → `1`.
4. Reject any character not in the Crockford alphabet
   `0123456789ABCDEFGHJKMNPQRSTVWXYZ` (no `I`, `L`, `O`, `U`).
5. Require **exactly 26 symbols**. Not "at least", not "up to".
6. Decode most-significant-bit first to 130 bit positions and **reject any input
   whose final two bits are non-zero**. Without this check a single secret has
   four distinct encodings, which makes "used once" ambiguous and gives an
   attacker four strings to try per guess.
7. The result is **exactly 16 bytes**. That byte string, never the text, is what
   is stored, hashed, compared and fed to HKDF.

**Comparison happens after decoding, on the 16 bytes, in constant time.** The
textual form is never compared, never used as a map key, and never logged.

**QR representation** encodes the same 128-bit value, as the canonical 26-symbol
form without hyphens, so a scanned secret and a typed secret decode to identical
bytes. There is no second format and no URI scheme to get wrong.

**Handling:** displayed by the installer on the server's console, and re-issuable
**only** by running `atriumctl pair` on the server. Core never transmits it, never
logs it, and never persists it in plaintext — only a hash, for attempt counting.
Single use, default 15-minute lifetime, consumed atomically on first successful
verification.

### 4. The exchange

Three messages over TLS. **Atrium requires TLS 1.3 for the pairing endpoints as a
deliberate minimum security baseline** — modern cipher suites only, no
renegotiation, a smaller handshake to reason about, and both endpoints are ours so
there is no compatibility argument against it.

That is the reason, and it is worth being accurate about what the reason is *not*.
Exporters are not a TLS 1.3 invention: RFC 5705 defined them for earlier versions
and RFC 8446 §7.5 carries them into TLS 1.3. The `tls-exporter` channel binding of
RFC 9266 is specified for TLS 1.3, and its use with TLS 1.2 depends on extended
master secret (RFC 7627) being negotiated — without it, the exporter is not
uniquely bound to the session and the binding does not hold. So TLS 1.2 with EMS
would be *possible*; requiring 1.3 removes the conditional, and that is a baseline
choice rather than a technical necessity.

```
C: pins spki = SHA-256(SPKI) from the TLS handshake
C -> S  GET  /api/v1/pair/info                      (unauthenticated, rate limited)
S -> C       { serverId, name, version, spki, claimed, pairingOpen }

C -> S  POST /api/v1/pair/begin  { deviceName, platform, clientNonce }
S -> C       { pairingId, serverNonce, expiresAt }

C -> S  POST /api/v1/pair/complete { pairingId, proofC }
S -> C       { proofS, deviceId, deviceToken, role, server }
```

Key derivation and proofs — standard primitives only, HKDF-SHA256 (RFC 5869) and
HMAC-SHA256 (RFC 2104):

```
cb   = TLS-Exporter("EXPORTER-Atrium-Pairing-v1", "", 32)      RFC 8446 §7.5, RFC 9266
K    = HKDF(secret16,                                          the DECODED 16 bytes, never the text
            salt = serverNonce,
            info = "atrium-pair-v1" ‖ serverId,
            len  = 32)

T    = serverId ‖ spki ‖ cb ‖ clientNonce ‖ serverNonce ‖ H(deviceName ‖ platform)

proofC = HMAC-SHA256(K, "atrium-pair-v1:client" ‖ T)
proofS = HMAC-SHA256(K, "atrium-pair-v1:server" ‖ T)
```

Every field in `T` is fixed-length (32-byte hashes and nonces, 16-byte `serverId`)
so the transcript cannot be made ambiguous by moving bytes between adjacent
fields. Comparisons are constant-time. The client **verifies `proofS` before it
stores the device token**; a server that cannot produce it is not the server the
owner has the secret for, and the client discards everything and reports
`untrusted`.

`clientNonce` and `serverNonce` are 32 bytes from a CSPRNG. `pairingId`,
`serverNonce` and the secret are each single-use; the `begin`→`complete` window is
two minutes, independent of the secret's 15-minute lifetime.

### 5. Why 128 bits, and what this construction is not

This is **not** a PAKE. An active attacker who terminates TLS in front of the
client obtains `proofC`, and can then brute-force the secret offline: the
server-side rate limit does not apply to an attacker's own CPU. The mitigation is
entropy, and it is only a mitigation because the secret is large enough — which is
why §3 fixes it at 128 bits and why the secret is copied or scanned rather than
typed.

**Short, human-typed codes are therefore not offered**, and cannot be added by
shortening the secret. Adding them requires adopting a reviewed PAKE —
**SPAKE2+ (RFC 9383)** or CPace — and that is a prerequisite recorded here, not an
aspiration. Matter uses SPAKE2+ for exactly this reason and it is the reference to
follow if the feature is ever wanted.

### 6. Claim on first pair

The first successful pairing creates the owner account and marks the server
claimed. The claim window is bounded; past it, claiming requires a freshly issued
console secret. A LAN attacker cannot win a race against a newly installed server
without reading its console output.

### 7. Credentials after pairing

- **Native clients** receive an opaque 256-bit device token, stored in the OS
  keychain, sent as `Authorization: Bearer`, hashed with Argon2id server-side,
  bound to a device record, individually revocable.
- **Browser clients** exchange the pairing result for an `HttpOnly; Secure;
  SameSite=Strict` session cookie plus a double-submit CSRF token, because a
  browser cannot hold a bearer token safely.
- Every device is listed with name, platform, first seen and last seen, and can be
  revoked immediately — not at expiry.

## What each mechanism actually provides

| Question | Answer |
| --- | --- |
| **What does the pairing secret authenticate?** | Access to the server's console or installer output. It proves the person pairing is, or was authorised by, whoever administers the machine. It authenticates a *person's authority*, not a device and not a user account. |
| **What does SPKI pinning prevent?** | A later impersonation. Once pinned, a different server — including one that obtained a valid certificate for the same address — is rejected. It is what makes identity survive address changes, since the pin is on the key, not the name. |
| **What does channel binding prevent?** | Relay. A machine-in-the-middle has two distinct TLS sessions, so its exporter value differs from the one the real server computes; `proofC` therefore does not verify upstream. Including `spki` as well means the pin the client is about to trust is itself covered by the proof. |
| **Replay resistance** | `cb` is unique per TLS connection; `clientNonce` and `serverNonce` are per-attempt; `pairingId` and the secret are single-use and consumed atomically. A captured `proofC` is worthless against any other connection. |
| **Expiry** | Secret: 15 minutes by default. Pairing session: 2 minutes from `begin`. Both are server-enforced; the client's clock is not trusted. |
| **Single use** | The secret is consumed on first successful verification, inside the same transaction that creates the device record. A crash between the two leaves the secret consumed, never the reverse. |
| **Rate limiting** | Per-IP token bucket on the whole pairing surface, plus a global counter: five failures disable pairing until it is re-armed on the server. This bounds online guessing; it does nothing about offline guessing, which is what §5 is for. |
| **Re-pairing** | A new secret from the console, a new device record, a new token. An existing device is unaffected. Re-pairing the *same* device produces a second device record, which the owner can see and prune. |
| **Key rotation** | `atriumctl rotate-identity` generates a new key pair. Every pin breaks, every device must re-pair, and the UI says so before it happens. Deliberately heavyweight, and the correct response to a suspected key compromise. |
| **Certificate reissue** | Automatic on address change, expiry, or hostname change. Same key pair, so no pin breaks and no client notices. |
| **Device revocation** | Immediate, per device, from any other paired client with the `owner` role. The token's hash is deleted and any live session is invalidated on the next request. "Revoke all" additionally forces re-pairing of everything. |
| **Losing every client** | `atriumctl pair` on the console issues a new secret. This is the one documented terminal exception in the product. |

## Attacker model

| Attacker | Outcome |
| --- | --- |
| Passive LAN observer | Sees TLS records and mDNS. Learns nothing; the proofs are inside TLS and the secret never crosses the wire. |
| Active LAN MITM without the secret | Can terminate TLS and capture `proofC`. Cannot relay it (channel binding), cannot produce `proofS`, so the client refuses. Can attempt offline recovery of the secret; infeasible at 128 bits. |
| Attacker with the secret but not on-path | Pairs successfully. The secret *is* the authority — this is by design, and is why it is console-only, single-use and short-lived. |
| Attacker racing a fresh install | Blocked by claim-on-first-pair plus the bounded claim window. |
| Attacker with an old, revoked device token | Rejected on the next request; revocation is immediate. |
| Attacker who compromises Core | Already holds every token hash and can mint devices. Out of scope for pairing; bounded instead by `SECURITY.md` §4. |
| Attacker who spoofs mDNS | Redirects the client to their own server. The client pins the wrong SPKI, then fails to verify `proofS` and reports `untrusted`. Discovery is never trust. |

## Consequences

**Good**

- No novel cryptography. HKDF, HMAC, TLS exporters and SPKI hashing are all
  standard, widely reviewed constructions; the only project-specific part is the
  transcript, which is specified byte-exactly above.
- DHCP, multi-homing, IPv6 and hostname collisions stop being trust problems.
- Mutual proof means a fake server is detected before anything is stored.
- MITM during pairing is defeated without a PKI.
- No CA, no account, no cloud, no internet required.

**Costs**

- **The secret must be copied, not remembered.** That is a real UX constraint, and
  the honest consequence of not using a PAKE. A short code is a feature request
  with a cryptographic prerequisite attached.
- **The browser still sees a self-signed certificate and warns** (A-24). The
  weakest point in the design; resolved before Beta per `SECURITY.md` §14.
- Pinning means key rotation is a re-pair of every device, which needs a UI clear
  enough that it is not mistaken for an attack.
- TLS 1.3 is required for the pairing endpoints. Every supported client platform
  has it, so the cost is low — but it is a deliberate baseline that is written
  down, not an assumption, and not a consequence of the channel binding.
- mDNS is blocked on some networks; manual entry must stay visible, never a hidden
  fallback.
- A bearer token in an OS keychain is stolen by a compromised client (T3). Device
  revocation and the audit log are the mitigation, not prevention.

## Alternatives considered

**SPAKE2+ (RFC 9383) or CPace for Alpha.** The cryptographically ideal answer, and
the only way to have short human-typed codes safely. Rejected for Alpha on
implementation-risk grounds: it needs a reviewed implementation and careful group
handling, and a 128-bit copied secret reaches the same security goal with standard
primitives. Recorded as the prerequisite if short codes are ever wanted.

**Hand-rolled channel binding over the SPKI hash alone** — the previous revision.
Replaced: the SPKI hash is stable across connections, so it binds to the server's
key but not to *this* session. The RFC 8446 §7.5 exporter, used as RFC 9266's
`tls-exporter` channel binding, is the standard answer and costs nothing to use.
Both the exporter and the SPKI hash are now in the transcript.

**Username and password.** Rejected for Alpha: adds a reset flow, a brute-force
surface and a credential to lose, for no gain over a one-time secret plus device
tokens. Passwords arrive with multi-user support, where they buy something.

**Mutual TLS with a Core-issued client certificate.** Attractive for revocation and
replay resistance, and still a candidate for native clients later. Rejected as the
Alpha mechanism: browser client-certificate UX is poor, and provisioning a
certificate through pairing is more moving parts than a token.

**Trust on first use with no secret.** Rejected: on a network assumed hostile
(A-07), first use is exactly what an attacker races for.

**Cloud-account-based pairing.** Rejected by ADR-006.
