# ADR-005 — Declarative app manifests as the application platform

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-002, ADR-004, ADR-013, ADR-014, ADR-016
**Assumptions:** A-13, A-14, A-15, A-16

## Context

The prototype integrates three services — Homarr, Glances, qBittorrent — and each
one cost a dedicated Rust module, a credential adapter, a response parser, a
validation state machine and its own UI surface. The code is good. The pattern is
fatal at catalogue scale: the fourth service costs the same as the third, and the
fortieth is impossible.

Meanwhile the product's promise is that a normal person installs Jellyfin in one
step and it keeps working. That requires the platform to own the app's lifecycle —
its container, ports, volumes, secrets, health, upgrades and data — not merely
link to it.

## Decision

**An application is a declarative manifest plus user settings plus
platform-allocated resources.** Core resolves those three into an install intent;
**Agent** verifies the manifest's signature, derives the container specification
from it and applies it (ADR-013, ADR-014). Adding an app to the catalogue is a
data change. If an app needs UI code, either the schema is missing a concept or
the app is not ready for the catalogue.

The manifest format is specified in [`../APP-SDK.md`](../APP-SDK.md). The
decisions that are frozen here:

1. **Containers are the only runtime kind in v1** (A-13). `runtime.kind` exists so
   a second kind is possible, but nothing else is designed for one.
2. **Images are pinned by digest.** A manifest without a digest does not validate.
   `latest` is not expressible. A version change is an explicit, visible manifest
   revision.
3. **Manifests contain no argv.** No `command`, no `entrypoint`, no `args`, no
   shell hooks. Lifecycle hooks are a closed set of typed operations
   (`backup-volumes`, `stop-app`, `wait-healthy`). This is ADR-004 applied to the
   app platform.
4. **Everything that widens the blast radius is declared — and in Alpha, refused.**
   Host paths, device mappings, host network, privileged mode, added kernel
   capabilities and access to Atrium's own API live under `permissions`. The block
   exists so the concept is designed, but Agent rejects every one of them and the
   schema requires them empty (ADR-014). They return only through a grant the
   owner makes at the server console, which Agent can verify without trusting
   Core — never through consent recorded in an API call, because a compromised
   Core would manufacture that consent.
5. **Agent is the sole writer of app containers**, and the only component that can
   write one at all (ADR-013). Agent names the container, stamps
   `io.atrium.managed=true`, `io.atrium.app=<id>` and an owner token Core never
   sees, and records it in its own registry. Containers the user created are
   visible and **read-only** — not start, stop or restart — until an explicit
   future adoption workflow exists (A-16).
6. **Secrets are generated and held by Core** and passed with the install intent,
   never present in the manifest and never returned by the API.
7. **Ports are allocated, not assigned.** `suggestedHost` is a suggestion; Core
   resolves conflicts against listening sockets and other apps and reports the
   result.
8. **Half-installed is not a resting state.** Every install step has a defined
   rollback; a failure reports which step failed and leaves a clean previous state
   or an explicitly `unhealthy` installed app with reachable logs.
9. **The catalogue is curated and first-party.** Signed with a **catalogue
   publisher key that is separate from any server's device identity and from the
   release signing key** (ADR-016), versioned, and bundled with the release in
   Alpha. No third-party publishing, no user-submitted manifests in
   the UI, no marketplace. A local manifest can be installed for development behind
   an off-by-default, clearly-unverified setting.

## Consequences

**Good**

- Catalogue growth is bounded by curation effort, not engineering effort.
- Install, upgrade, uninstall, backup and health are one implementation shared by
  every app, so they get to be good.
- Permissions become visible to the owner instead of being implicit in someone
  else's compose file.
- The schema is a forcing function: an app that needs an escape hatch exposes a
  real gap in the platform rather than hiding it in bespoke code.

**Costs**

- Real applications resist declaration. Anything needing an init script, a
  first-run wizard or a sidecar will not fit v1, and the honest response is to
  leave it out of the catalogue rather than to add a hook that runs a script.
- **Hardware transcoding is not available in Alpha at all** (A-14, ADR-014).
  Device mapping is part of the refused permission set, so Jellyfin transcodes on
  the CPU. This is the most user-visible price of the security position, and it is
  paid deliberately rather than worked around with a flag.
- **The catalogue is small on purpose.** Beta targets roughly ten curated
  first-party applications (A-15), not a large public marketplace. Ten is a
  curation commitment, not a ceiling imposed by the design.
- Multi-container applications are only minimally expressible in v1. An app with a
  database is a design gap, deferred deliberately.
- Digest pinning means the catalogue must be rebuilt for every upstream security
  update, and a withdrawn image breaks installs. The catalogue build needs
  monitoring that does not exist yet.
- Curation is an ongoing cost borne by this project, in perpetuity.

## Alternatives considered

**Docker Compose files as the unit of installation.** Familiar, and every app
already ships one. Rejected: compose is an execution format, not a description —
it carries commands, arbitrary host paths, arbitrary volumes and privileged flags
with no consent model, and it gives the platform no way to know what an app needs,
what to back up, or what changed in an upgrade. Compose *import* for existing
stacks is a reasonable later feature; compose as the platform's data model is not.

**Per-service code, as the prototype does.** Rejected: measured, and the measurement
is in `PROTOTYPE-ASSESSMENT.md` §2.3.

**Helm-style templating.** Rejected: templating is a programming language wearing a
data format's clothes, and it reintroduces everything ADR-004 removes.

**A public marketplace from the start.** Rejected: signing, review, trust,
revocation and abuse handling are a product of their own, and the catalogue is not
the bottleneck at this stage.

**Installing apps as host packages rather than containers.** Rejected: version
conflicts, no isolation, no clean uninstall, no per-app data boundary, and a
different answer on every distribution.
