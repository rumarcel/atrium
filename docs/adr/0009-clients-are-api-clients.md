# ADR-009 — Every client is an unprivileged API client

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-001, ADR-003, ADR-008, ADR-010

## Context

The existing desktop application holds the product's logic: the service catalog,
the credential vault, every HTTP client, the trust policy, the validation rules.
It is, in effect, a privileged client — the only thing that knows how to talk to
the server, and the only place the configuration exists.

With Core in place, that arrangement would produce two sources of truth, two
implementations of every rule, and a web client permanently behind the desktop
one. It would also mean the product's behaviour depends on which client you
happen to be using, which is the thing platform products get wrong most often.

## Decision

**One public API, no privileged client channel, no second API.** Every client —
web, Windows, macOS, Linux, mobile, TV — speaks `/api/v1` and nothing else.

- **The web client is the reference implementation**, bundled into the Core binary
  and served by it. Anything a native client can do, the web client can do.
- **A client holds exactly one thing of value**: its device token, in the OS
  keychain. No credentials for third-party services, no privileged configuration,
  no trust policy of its own beyond the pinned SPKI.
- **Business rules live in Core.** A client may validate for responsiveness; the
  server validates for truth. Where the two disagree, the server is right.
- **Role knowledge is for rendering only.** Authorization is enforced at the route
  (ADR-004, `SECURITY.md` §7). A client that renders a control it should not have
  gets a `403`, not an effect.
- **The desktop client keeps what a browser genuinely cannot do**: OS keychain
  storage, background presence and tray lifecycle, native notifications, desktop
  cards, embedded service WebViews with persistent per-service profiles, startup
  registration, and the per-monitor DPI and window-geometry handling that already
  works.

### What this means for the existing application

| Component | Disposition |
| --- | --- |
| Shell, tabs, WebView hosting, fullscreen handling, DPI/bounds observer | keep unchanged — genuinely good, genuinely native |
| Appearance engine, theme packs, i18n catalogs | keep unchanged |
| Tray, close-to-tray, single instance, startup, notifications, desktop cards | keep, re-pointed at Core's event stream |
| Service catalog ownership and its atomic-save/recovery logic | move to Core as external links; the validation logic ports |
| Glances, Homarr, qBittorrent HTTP clients | retire from the client; the platform supplies system data natively and the rest become integrations behind the app platform |
| Credential vault | split — the device token stays in Credential Manager; service secrets move to Core's secret store |
| Health checking | move to Core; the client displays results |
| Server control (enrollment, power, confirmations) | replaced by Core's operations and confirmations |

Migration is offered, not imposed: on first run against a Core, the desktop client
offers to import its local catalog as external links and then stops being the
source of truth. Nothing is deleted from the old location.

## Consequences

**Good**

- One implementation of every rule, in the component that can enforce it.
- A second client is a UI project, not a re-implementation of the product.
- Two clients cannot disagree about state, because neither of them owns it.
- The security boundary is simple to state: a compromised client is one revocable
  device (`SECURITY.md` T3), not a hole in the platform.
- The desktop app gets smaller and easier to port to macOS and Linux, because the
  parts that were hardest to port are the parts that move to Core.

**Costs**

- **Large, visible deletions from working code.** Several thousand lines of
  carefully written, tested Rust in the desktop client stop being used. That is
  emotionally and practically harder than adding code, and it will be tempting to
  keep "just the Glances client" as a fallback. It should not be kept.
- Every client action becomes a network round trip. On a LAN this is
  imperceptible; over WireGuard it is not, and the UI must be built for latency
  from the start.
- The desktop client is useless without a paired Core, which is a real regression
  for the current single user, who today needs no server component at all.
- Feature work now requires touching Core and at least one client, which is more
  coordination than a single process.

**Neutral**

- The web client being the reference means it must be built early — it is M4 of
  Alpha 0.1, not a later addition.

## Alternatives considered

**Keep the desktop client's local integrations as a fallback when no Core is
paired.** Superficially kind to the existing user. Rejected: it preserves two
implementations of everything forever, and the fallback would be the one nobody
tests.

**A privileged desktop channel with capabilities the web client lacks.** Rejected:
it makes the web client second-class permanently and creates a second
authorization surface.

**Client-side business logic with Core as a data store.** Rejected: authorization
and auditing cannot live in a client, and every client would have to be trusted
equally.

**Rewrite the desktop client from scratch as a thin shell.** Rejected: its shell,
tab system, appearance engine and Windows integration are assets worth more than
the cost of re-pointing them (A-27).
