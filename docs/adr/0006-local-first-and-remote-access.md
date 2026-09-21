# ADR-006 — Local-first, with optional tiered remote access

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-003
**Assumptions:** A-02, A-07

## Context

Competing products in this category increasingly require an account, a cloud
relay, or a mobile app that will not function without a vendor's servers. That
choice buys easy remote access and costs the user their independence: when the
vendor stops, the hardware they own stops working properly.

Atrium's users are choosing self-hosting specifically to avoid that. The
prototype's README makes the commitment explicitly — no account, no relay, no
telemetry, no phone-home — and that commitment is part of the product, not an
implementation detail.

Remote access is still a real need. A media server nobody can reach from outside
the house is half a product for many owners.

## Decision

### Local-first is a hard guarantee

- Every capability works with the internet physically disconnected. This is
  verified in the Alpha acceptance run (`ROADMAP.md` M1, criterion 33).
- No account is required, ever, for any function.
- No telemetry. No analytics endpoint. No crash reporting that leaves the machine
  without an explicit, per-incident action by the owner.
- The only outbound connection Atrium initiates on its own is an update check,
  which is disableable and sends nothing beyond a version string.
- A cloud service is never a dependency of a LAN feature. If one is ever built, a
  LAN-only installation must lose nothing but remote access.

### Remote access is off by default and tiered

| Tier | Mechanism | Posture |
| --- | --- | --- |
| 0 | LAN only | default; nothing listens beyond the local network |
| 1 | The owner's existing VPN | documented, zero code, recommended |
| 2 | Atrium-managed WireGuard | Core generates peer config and a QR code; no service is exposed publicly |
| 3 | Direct exposure | supported only with repeated warnings, a forced review of what becomes reachable, a real certificate, stricter rate limits and optional second factor |
| 4 | A hosted relay | **not built.** If ever built: opt-in, end-to-end encrypted, zero-knowledge of content, never required |

### Remote access never widens the API

Same routes, same roles, same audit, same operations. There is no "remote mode"
that unlocks or restricts anything. A request is authenticated and authorized
identically regardless of where it came from — which is also the honest position,
because the LAN is not trusted either (A-07).

## Consequences

**Good**

- The product keeps working if this project stops. That is worth more to the
  target user than any feature.
- No infrastructure to run, secure, pay for or be legally responsible for.
- No privacy policy that needs to explain what is collected, because nothing is.
- The threat model stays small: no relay means no relay compromise.
- Tier 1 costs nothing to support and is the safest option, so the recommended
  path is also the cheapest one.

**Costs**

- Remote access is harder for the user than a vendor cloud. A relay would be one
  tap; WireGuard is a configuration the owner must apply on each device.
- Without a relay, NAT and CGNAT defeat tiers 2 and 3 for some users, and Atrium
  can only explain why rather than solve it.
- No crash telemetry means bugs are found by users reporting them, which is
  slower. The diagnostics bundle is the compensation and it has to be good.
- Update checks that are disabled mean some installations never learn about
  security fixes. Documented honestly rather than solved by forcing the check.

**Neutral**

- Tier 4 is left explicitly unbuilt rather than declared impossible, so that if it
  is ever built the constraints are already recorded.

## Alternatives considered

**Mandatory account with a relay.** Rejected: it is the thing the users are
leaving.

**Optional account that unlocks features.** Rejected: it makes the account the
real product and the local mode the degraded one, which is the same outcome by a
slower route.

**Ship a relay as self-hostable software.** Interesting, and not incompatible with
this ADR. Deferred: it is a second product.

**Rely on third-party tunnels (Tailscale, Cloudflare Tunnel) as the answer.**
Rejected as *the* answer, welcomed as *an* answer — these are exactly what tier 1
documents. Atrium will not require one, will not bundle one, and will not make its
own features depend on one.
