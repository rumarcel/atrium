# ADR-020 — A failed pairing proof consumes its attempt and nothing else; there is no five-failure lock

**Status:** accepted (2026-09-24, before M1F). Supersedes the "five failures
disable pairing until it is re-armed" rule of ADR-003's *What each mechanism
actually provides* table (as restated in plan §6.1 and §6.5, SECURITY.md §6
item 5 and §13, criterion 13, and ADR-019's lifetime-table row "fifth failed
proof"), and nothing else
**Date:** 2026-09-24
**Related:** ADR-003, ADR-019
**Supersedes:** the five-failed-proof lock only

## Context

ADR-003 kept a counter of failed proofs per armed secret, and at the fifth
destroyed the secret and locked pairing until the owner re-armed it at the
console. M1E built exactly that: `pairing_state.failures` and
`pairing_state.locked`, the `pairing.locked` audit action, and a `CHECK` that
tied the two to the armed shape.

That rule is borrowed from short-PIN pairing, where a handful of online
guesses is a real fraction of the space. Atrium's secret is not a PIN
(ADR-003 §3, §5):

- **Online guessing of a 128-bit uniformly random secret is not a practical
  attack.** At the pairing surface's global ceiling of 60 attempts a minute,
  the expected work is about 2¹²⁷ attempts — about 5 × 10³⁰ years — whether or not
  anything locks after five. A secret lives 15 minutes, so an attacker gets
  at most roughly 930 tries against any one secret: a success probability
  near 2⁻¹¹⁸.
- **A stolen secret succeeds on its first correct use.** Whoever has the
  secret does not need a second try, so a five-attempt lock stops nothing
  that has the secret.
- **What the lock actually did** was hand every unauthenticated LAN client a
  denial-of-service primitive: five bogus `complete`s — no knowledge of the
  secret needed, each only needing a `begin` of its own — terminated the
  owner's pairing window and forced a trip back to the console. The M1E
  review listed that as a residual risk; it is the lock's main effect.

## Decision

1. A `pair/complete` that fails — wrong proof, other connection, window
   passed — **consumes that one attempt** (it was already single use), is
   answered with the generic `403 pairing.rejected`, and increments a
   per-arming counter, `pairing_state.failed_attempts`.
2. It **does not** destroy, lock or otherwise change the armed secret. The
   secret stays usable until exactly one of:
   - a successful pairing consumes it;
   - its 15-minute validity ends;
   - the owner replaces it with `atriumctl pair`;
   - another existing console action invalidates it (`atriumctl
     rotate-identity`, which revokes everything and arms a new one, or the
     identity-change revocation at Core's start).
3. The counter is telemetry, not policy. It is never returned by any API
   route, never used to decide anything, and is recorded in the audit row
   that ends the arming (`pairing.consumed`, `pairing.expired`, or the
   `pairing.armed` row that replaced it) as `failed_attempts`. Individual
   failures are logged (`pairing_rejected`, with a closed reason) but not
   written to the audit table one by one, so an attacker cannot grow the
   audit table faster than one row per console arming.
4. Everything else stands: the TLS-exporter and SPKI binding, single-use
   attempts, the two-minute attempt window, at most four attempts in memory
   and one per source, the per-source and global rate limits, and one
   indistinguishable refusal for every failure.
5. The rate limits stay, for what they are actually for: **bounding the
   work and state an unauthenticated client can cause**, not protecting the
   secret's entropy, which needs no protection at this scale.

Migration 3 removes `failures` and `locked` from `pairing_state` and adds
`failed_attempts` (zero whenever nothing is armed). A row that was locked is
already disarmed, so it comes across as simply disarmed; an armed row keeps
its sealed secret (ADR-019's associated data never included the counter)
with its count carried over. The migration runs behind the usual backup.

## Consequences

- A LAN attacker can no longer end the owner's pairing window. It can still
  spend the global pairing budget or occupy attempt slots for up to two
  minutes each; both slow the owner down, neither ends the window, and both
  recover without the console.
- Criterion 13 changes from "5 failures lock" to "failed attempts are
  consumed; the armed secret is unaffected" (ROADMAP, *criteria amended*).
- The `pairing.locked` and `pairing.failed` audit actions are no longer
  written. Rows written by M1E remain valid history.

## Alternatives considered

- **Raise the threshold (for example to 1 000).** Still a denial-of-service
  primitive, only slower; still protects nothing. Rejected.
- **A per-source lock.** Sources are addresses, which a LAN attacker can
  multiply; it would only block the owner if they share an address with the
  attacker. The per-source rate limit already bounds the cost. Rejected.
- **Keep the counter in memory only.** Loses the telemetry at a restart
  inside the window; the one extra `UPDATE` per failure is bounded by the
  rate limits. Rejected.
