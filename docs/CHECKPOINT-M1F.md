# Checkpoint — after M1F

**Status: M1F COMPLETE · M1G NOT STARTED · PROJECT INTENTIONALLY PAUSED**

| | |
| --- | --- |
| Checkpoint date | 2026-09-24 |
| Branch | `product/m1-alpha` |
| Product code at | `5337ba0` — last implementation/docs commit of M1F |
| M1F commits | `c769dbf` (ADR-020, no failure lock), `8936610` (providers, routes, tests), `5337ba0` (docs as built) |
| Green CI for that code | run `35944210600` on `5337ba0` — all four jobs green, every Server-workspace step including the privileged suite |
| Checkpoint commit | the commit adding this file (documentation only); tag `m1f-checkpoint` points at it |
| Earlier checkpoint | `m1a-checkpoint` ([CHECKPOINT-M1A.md](CHECKPOINT-M1A.md)), unchanged |
| **NEXT** | **M1G — discovery and desktop client integration** |

The pause is deliberate. Nothing is half-finished, blocked or waiting on a
decision.

## Milestone state

| Pass | State | Delivered |
| --- | --- | --- |
| M1A | complete | Rust workspace under `server/`, Core and Agent skeletons, architectural gates in CI |
| M1B | complete | install-time identity (ADR-017), certificate reissue on the same key, SQLite state with backup-before-migrate, recovery mode, `atriumctl restore` / `rotate-identity` |
| M1C | complete | Core↔Agent typed protocol (two parameterless read ops, passive runtime probe), peer credentials, Agent journal, `agent.call` audit |
| M1D | complete | HTTPS-only listener (rustls, HTTP/1.1), literal route table with per-route policy, Host allowlist, deny-by-default Origin, RFC 9457 errors, recovery network subset |
| M1E | complete | console-armed pairing bound to TLS exporter + SPKI, sealed secret (ADR-019), device tokens (SHA-256 verifier), devices/revocation, per-source limits |
| M1F | complete | ADR-020 (no failure lock); native providers behind ADR-002 traits; `/system`, `/system/metrics`, `/system/capabilities`, `/system/diagnostics` (normal mode), `/network/interfaces`, `/storage/filesystems` |
| M1G | **not started** | discovery and desktop client integration |
| M1H, M1I | not started | installer and units in anger; acceptance hardening |

Per-pass detail, divergences and security reviews: the *As built* entries in
[M1-IMPLEMENTATION-PLAN.md](M1-IMPLEMENTATION-PLAN.md) §17. Test evidence:
the *As built* mappings in [M1-TEST-PLAN.md](M1-TEST-PLAN.md).

## Security invariants in force

- Core runs as `atrium`, never root; Agent is the only root component, with
  a closed, parameterless operation set and no HTTP/TLS/SQL/runtime client.
- Identity material is root-owned (`root:atrium 0640`); Core reads, never
  writes or generates it. `secrets.key` never enters the database.
- HTTPS only; Host allowlist; no CORS; every device route passes one central
  `Auth::Device` check; unknown and revoked tokens are indistinguishable;
  revocation is immediate (no auth cache).
- Pairing: 128-bit console secret, TLS 1.3 only, bound to the connection's
  exporter and the server's SPKI, same-connection `begin`/`complete`,
  single-use attempts, 2-minute attempt window, 15-minute secret, generic
  `pairing.rejected`, no profile negotiation. Secret stored only sealed
  (ADR-019).
- **ADR-020 (current pairing behaviour):** there is no five-failure lock. A
  failed attempt consumes only that attempt. The armed secret stays usable
  until successful pairing, expiry, explicit re-arm at the console, or
  identity invalidation. Rate limits remain for resource control.
  `failed_attempts` is observational audit metadata only, never a decision
  input and never served by the API.
- Recovery mode: no pairing, no device auth, no Agent, no mutation; a smaller
  unauthenticated diagnostics allowlist.
- Providers never invent values: unreadable is `null` + a closed reason code.
- No third-party monitoring (Glances, Homarr, Cockpit, …) in the server path —
  enforced by gates.

## Known limitations (true at this checkpoint)

- Agent-derived capabilities may be up to 5 minutes old; they carry `checkedAt`.
- `discovery` capability reports `not_yet_implemented` until M1G.
- systemd unit validation (`systemd-analyze verify`) is informational in CI
  (`continue-on-error`); unit content is enforced by the deployment-policy test.
- The two-NIC privileged test checks IPv6 wherever the kernel has it and
  requires it under CI (runner has IPv6); a kernel without IPv6 checks IPv4
  only and says so.
- No metrics history (`metrics.db` not created); no final UI; no App Center
  implementation; no remote or cloud dependency.
- `revoke-all` awaits the confirmation mechanism; the device list is not
  paginated (operator-bounded).
- Plan L6 VM scenarios for criteria 21–25 and the diagnostics-bundle half of
  criterion 32 belong to M1H/M1I; M1F's evidence is the privileged real-kernel
  suite, as mapped in the test plan.

## Accepted residual risks

- A LAN client can spend the global pairing budget or hold attempt slots for
  two minutes (slows, never ends, the owner's window).
- Transient copies inside the HTTP/TLS stack are not all zeroized
  ([SECURITY.md](SECURITY.md) §20 item 8).
- Network timing of pairing refusals is comparable, not claimed constant.
- The remaining residual risks in [SECURITY.md](SECURITY.md) §20.

## Product direction (authoritative documents — not restated here)

- **What Atrium is:** a hardware-independent personal-server platform that
  brings a polished, appliance-like experience to hardware the user already
  owns — easy to install, easy to manage, understandable when something goes
  wrong. See [PRODUCT.md](PRODUCT.md).
- **UI:** development UI is allowed during Alpha; final UX/design is deferred
  to a dedicated design phase before public Beta; once frozen, it is
  implemented, not reinvented. UGREEN UGOS Pro and Synology DSM are
  experience-quality references, not designs to copy.
  See [UI-DESIGN-STRATEGY.md](UI-DESIGN-STRATEGY.md).
- **App Center:** manifest-driven, curated, about ten well-maintained
  first-party/verified apps for early Beta, no public marketplace at first,
  existing user containers read-only until explicitly adopted.
  See [PRODUCT.md](PRODUCT.md) *App Center direction* and [APP-SDK.md](APP-SDK.md).
- **Licensing/commercialization:** intentionally undecided; options preserved;
  neither fully open source nor fully proprietary is assumed.
  See [PRODUCT.md](PRODUCT.md) *Commercialization and licensing policy*.
- **Prototype:** the Windows desktop app in `src-tauri/` remains the working
  prototype, untouched; Alpha runs as a canary beside it (ADR-015). The
  prototype's third-party integrations stay with the prototype.

## Resume procedure

NEXT: M1G — discovery and desktop client integration

Do not skip to M1H. Before writing any M1G code, a fresh session should:

1. fetch and pull `product/m1-alpha`;
2. read this file;
3. read [ROADMAP.md](ROADMAP.md), [M1-IMPLEMENTATION-PLAN.md](M1-IMPLEMENTATION-PLAN.md)
   (§10 discovery, §17 M1G) and [M1-TEST-PLAN.md](M1-TEST-PLAN.md);
4. run CI and the regression suites (including the privileged suite);
5. check dependency and security advisories — assess updates deliberately;
   do not upgrade merely because time has passed;
6. confirm the accepted ADRs (001–020) still match the implementation;
7. only then begin M1G.
