# ADR-003 — Server identity, SPKI pinning and code-based pairing

**Status:** accepted — revised in the hardening review of 2026-09-22; §1's
generation and file-mode sentences superseded by [ADR-017](0017-install-time-identity-and-root-owned-key-material.md)
**Date:** 2026-09-22
**Related:** ADR-006, ADR-009
**Assumptions:** A-06, A-07, A-08, A-24, A-30, A-31

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

A third review asked whether the exporter binding creates a dead end for the
browser client that the roadmap promises. It does not, but only because the
transcript now carries an explicit **binding profile** (section 4a): the native
profile is what M1 implements, the browser profile is additive, and the browser's
real obstacle is its TLS trust anchor rather than this protocol. The same review
superseded Argon2id for device tokens (section 7).

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

> **Superseded in part by [ADR-017](0017-install-time-identity-and-root-owned-key-material.md).**
> Identity is created at installation by `atrium-core init-identity`, not by
> Core at first start, and the files are `root:atrium 0640` in a
> `root:atrium 0750` directory, not `0600`. The rest of this section — what
> `server_id` is, reissue from the same key, SPKI pinning — stands. The text
> below is kept as written.

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

prof = SHA-256(binding_profile_id)                             32 bytes, see section 4a

T    = prof ‖ serverId ‖ spki ‖ cb ‖ clientNonce ‖ serverNonce ‖ H(deviceName ‖ platform)

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

### 4a. Binding profiles, and why the browser is not a dead end

Browser JavaScript cannot read a TLS exporter and cannot read the peer
certificate. `cb` and `spki` are therefore computable by a native client and by
nothing that runs in a page. A design that assumed otherwise would have to be
replaced the day the web client arrives, which is exactly the outcome the product
forbids.

The transcript therefore carries an explicit **binding profile**, hashed into `T`
so two profiles can never produce the same proof and a profile can never be
silently reinterpreted.

| Profile id | Used by | `spki` | `cb` | Where MITM resistance comes from |
| --- | --- | --- | --- | --- |
| `atrium-pair-binding/native-tls-exporter-v1` | native clients — **the only profile M1 implements** | SHA-256 of the handshake SPKI | RFC 8446 exporter | the binding itself: a relay has a different key and a different exporter |
| `atrium-pair-binding/web-pki-v1` | a future browser client | 32 zero bytes | 32 zero bytes | **the browser's TLS trust anchor**, not the pairing exchange |

Everything else is shared: one secret format, one key schedule, one proof
structure, one device model, one token, one revocation path. Adding the second
profile is an additive change to a field that already exists, not a replacement
protocol. That is the whole point of putting the field in now.

**The honest part.** `web-pki-v1` is strictly weaker than the native profile, and
the weakness is not in the pairing exchange — it is that a browser reaching a
self-signed server has no trust anchor at all, so anything it does is
click-through security. The browser's problem is the certificate, not the
protocol. It is therefore gated on the same decision A-24 already records: a
locally installed Atrium CA, or an ACME certificate for a real name. Until one of
those exists there is no honest browser pairing, and `web-pki-v1` stays
unimplemented.

**Downgrade is impossible without a console action.** A profile is not negotiated.
Each armed secret records which profiles it permits; `atriumctl pair` arms
`native-tls-exporter-v1` only. Browser pairing requires
`atriumctl pair --allow-browser`, which prints what that profile does and does not
protect against and refuses unless Core is configured as serving a
PKI-trusted certificate — an operator assertion made at the console, because Core
cannot observe what a browser trusts. A man-in-the-middle cannot ask for the
weaker profile, because the client does not choose it and the server will not
accept it.

**If the certificate question is ever answered badly**, the fallback is a real
augmented PAKE — **SPAKE2+, RFC 9383** — which would give a browser MITM
resistance without any TLS visibility. It is recorded as the named fallback and
deliberately not adopted now: it is not needed for native clients, the mature
Rust implementations of SPAKE2+ specifically are thin, it would add a WASM crypto
bundle to the web client, and it solves offline guessing, which a 128-bit secret
does not have. Adopting it would be paying a large complexity cost against a
threat this design does not face.

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
  keychain, sent as `Authorization: Bearer`, bound to a device record,
  individually revocable. The server stores **`SHA-256(token)`** and nothing
  else — see the note below.
- **Browser clients** exchange the pairing result for an `HttpOnly; Secure;
  SameSite=Strict` session cookie plus a double-submit CSRF token, because a
  browser cannot hold a bearer token safely.
- Every device is listed with name, platform, first seen and last seen, and can be
  revoked immediately — not at expiry.

**Superseding note on the token verifier (2026-09-22).** An earlier revision of
this ADR specified Argon2id. That was wrong for this threat model and is
superseded. A device token is 256 bits from the OS CSPRNG; it is not a password,
is never chosen by a human, and has no guessable distribution. A password hash
buys resistance to offline guessing of a low-entropy secret, which does not exist
here. What it does buy is per-request latency, a verification cache to hide that
latency, the consistency bugs a cache invites on revocation, and an
attacker-triggerable CPU cost on an unauthenticated-adjacent path.

The verifier is therefore `SHA-256(token)`, compared in constant time. Requirements
that come with it: the raw token exists only on the client; the server stores only
the digest; the digest is deleted transactionally on revocation, with no cache
outliving the transaction; tokens are never logged; and an unknown token and a
revoked token produce an identical response.

A keyed verifier (HMAC with a server-side key) was considered and rejected: it
would protect against an attacker who can read the database but not the key file,
and in this deployment both live on the same disk under the same service
identity, so the key would sit next to the thing it protects. It adds a key to
manage and rotate in exchange for no threat-model benefit.

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
- **The browser still sees a self-signed certificate and warns** (A-24). This is
  now the single decision that gates browser pairing as well (section 4a), which
  raises its priority: it is no longer only a wart on the first-run experience,
  it is the prerequisite for the web client having any honest onboarding at all.
  Resolved before Beta per `SECURITY.md` §14.
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

**SPAKE2+ (RFC 9383) or CPace for Alpha.** The cryptographically ideal answer: it
would give short human-typed codes *and* a browser MITM resistance that needs no
TLS visibility, which would make section 4a's second profile unnecessary.
Rejected for Alpha on implementation-risk grounds: it needs a reviewed
implementation with careful group handling, the mature Rust implementations of
SPAKE2+ in particular are thin (the widely used `spake2` crate is balanced SPAKE2,
not the augmented RFC 9383 variant), a browser client would have to ship a WASM
crypto bundle, and the property it is famous for — offline-guessing resistance —
is one a 128-bit copied secret does not need. It is recorded as the named
prerequisite for short codes **and** as the named fallback if the browser
certificate question in A-24 is answered badly. Adopting it because a crate exists
would be the wrong reason; adopting it because the trust-anchor path fails would
be the right one.

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
