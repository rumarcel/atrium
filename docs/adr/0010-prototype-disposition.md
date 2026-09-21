# ADR-010 — What the prototype contributes, keeps and retires

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-001, ADR-004, ADR-009, ADR-011

## Context

The repository contains a finished, working, documented Windows application with
17,000 lines of Rust, a React frontend, a restricted Linux agent, a real test
suite and CI. It solves a different problem from the one the platform solves, but
it solves it carefully, and several of its decisions are better than what a fresh
start would produce under deadline.

Two failure modes are available. Preserving it wholesale would drag a
client-centric, Windows-coupled, IP-addressed architecture into the platform.
Discarding it would throw away security patterns that took real thought and are
already tested — and would break the one person currently using it.

## Decision

**The prototype is treated as a reference implementation and a running product,
not as the foundation of the platform.** Platform work happens alongside it in new
directories, not by rewriting it.

### Kept, unchanged

The desktop shell, tab system, native WebView hosting, fullscreen handling, DPI
and bounds observation, appearance engine and theme packs, i18n catalogs, tray and
background runtime, desktop cards, Windows startup and packaging, CI structure and
test conventions. These are client concerns and they stay client concerns
(ADR-009).

### Kept as patterns, reimplemented in Core and Agent

These are the prototype's real contribution and they are adopted as platform-wide
rules rather than as code:

| Pattern | Where it goes |
| --- | --- |
| Fixed argv, `shell=False`, fixed `PATH`, closed stdio, timeout | Agent operation execution (ADR-004) |
| Durable intent committed before any privileged dispatch | Operation resources (ADR-008) |
| A lost result stays `unknown` and is never retried automatically | Operation resources |
| Reconciliation against observed evidence (boot identity) rather than assumption | Operation resources |
| Strict parsing: unknown fields rejected, duplicate keys rejected, bounded bodies and headers | API contract, manifest validation |
| Closed enums for every state and reason; never upstream error text | API error model |
| Origin-bound credentials; `{ kind, exists }` and nothing more | Secret store, API rules |
| `Host` allowlisting and `Origin` rejection | Core's HTTP layer |
| Capability-scoped IPC with caller verification | Role checks, client boundaries |
| The frontend never supplies a URL; the backend resolves targets by id | API rule (`API.md` §8) |
| Atomic write, keep-previous-as-backup, visible recovery notice | Core's state store (ADR-012) |
| Honest unavailability: no invented values, no polling an endpoint that cannot exist | `UX-STATES.md` |
| Dry-run before a destructive capability is armed | Privileged operations and destructive confirmations |

### Relocated

- Service catalog → Core's external links, with its validation logic ported.
- Credential vault → split between the OS keychain (device token) and Core's
  secret store (everything else).
- Health checking → Core.

### Retired

- **The Python power agent.** Superseded by Agent. Its code is not carried
  forward; its patterns are, above. The sudoers model is removed entirely
  (ADR-004).
- **Glances as the metrics source.** Core reads the system natively. Glances may
  remain a linkable service.
- **The client-side Homarr, Glances and qBittorrent modules.** Homarr survives as
  an optional import source for external links; qBittorrent returns later as a
  catalogue application.
- **IP-based server enrollment.** Replaced by `server_id` and SPKI pinning
  (ADR-003).

### Immediate hygiene

The bundled seed catalogue's twelve entries point at the author's real LAN address
and hard-code a single-host topology. They are replaced with documentation-range
examples (`192.0.2.0/24`), as the prototype's own README already instructs. The
same literal in the systemd units, the agent README's `openssl` command and the
Python tests is replaced with a placeholder that is obviously a placeholder.

### Compatibility promise

The existing installation keeps working until the desktop client is migrated, and
the migration imports rather than replaces: it offers to bring the local catalog
across as external links and leaves the original files untouched.

## Consequences

**Good**

- The platform starts with a set of security patterns that are already tested and
  already survived contact with a real deployment.
- The one existing user is not broken.
- The prototype remains a working answer to "what does good look like here" for
  strictness, honesty and refusal-to-guess.

**Costs**

- Two codebases in one repository for a period, with duplicated concepts (health
  checking exists in both until the client migrates) and a build that covers both.
- Retiring the Glances, Homarr and qBittorrent modules deletes working, tested
  code, which is uncomfortable and will be argued about.
- The desktop client's migration is real work that produces no new user-visible
  capability, which makes it easy to postpone past the point where it should have
  happened.

**Neutral**

- The root `ROADMAP.md` stays as the prototype's history. `docs/ROADMAP.md` is the
  forward plan. Two roadmaps is mildly confusing and both say so at the top.

## Alternatives considered

**Rewrite from scratch in a new repository.** Rejected: loses the security
patterns, loses the CI, loses the documentation register, and abandons the current
user.

**Grow the desktop app into the platform.** Rejected by ADR-001 — a client cannot
be a platform.

**Freeze the prototype immediately and start clean here.** Rejected: the prototype
is the only working thing in the project and keeping it alive costs little while
the platform is unproven.

**Extend the Python agent into Core.** Rejected by ADR-007.
