# ADR-002 — Provider/adapter architecture with runtime capability negotiation

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-001, ADR-008, ADR-013

## Context

Alpha targets Debian and Ubuntu with systemd and Docker. The product promises
hardware and OS agnosticism, and the plausible future targets — Fedora, Rocky,
Alma, Podman, aarch64 boards — differ in package manager, container runtime and
sensor availability, but not in what the user is trying to do.

The failure mode to avoid is visible in the prototype: integration logic written
per service, with the caller knowing which one it is talking to. Repeated at the
OS level, that produces `if debian { apt } else if fedora { dnf }` scattered
through the codebase, and a UI that breaks on any machine nobody tested.

A second failure mode is assuming capability from identity. "This is Debian,
therefore SMART works" is false on a USB-attached disk. "This is Linux, therefore
there are temperature sensors" is false on most virtual machines.

## Decision

**Six provider traits**, each abstracting one domain: `PackageProvider`,
`ServiceProvider`, `ContainerProvider`, `StorageProvider`, `NetworkProvider`,
`HardwareProvider`.

No layer above the provider layer — API handlers, the app engine, the UI — may
name a distribution, a package manager, an init system or a container runtime.
Those names appear only inside adapters and in diagnostics.

**The traits are defined once; their implementations live on whichever side of
the privilege boundary can actually perform them (ADR-013).** `HardwareProvider`,
`NetworkProvider` and the read-only part of `StorageProvider` execute inside
Core, because they only read `/proc`, `/sys` and netlink. `ContainerProvider`,
`ServiceProvider`, `PackageProvider` and privileged storage execute inside
**Agent**, and Core reaches them through the typed operation protocol rather than
through the trait directly. This is not a layering compromise: the trait is the
vocabulary, and the protocol is how a vocabulary crosses a privilege boundary
without becoming a passthrough.

**Capability negotiation is runtime, not compile-time and not OS-derived.** Each
provider reports what it can actually do on this host, probed at startup and
re-probed when the environment changes. `GET /api/v1/system/capabilities` is the
single source of truth for clients, and it reports missing features with a reason
code, never by omission:

```jsonc
"storage": { "available": true, "provider": "linux", "features": ["filesystems"],
             "missing": [{ "feature": "smart", "reason": "no_smart_capable_device" }] }
```

**Provider selection is a detection pass**, recorded in diagnostics: read
`os-release`, identify PID 1, probe for runtime sockets and binaries, verify with
a cheap call rather than trusting the file. A failed detection produces a degraded
Core that still serves identity, pairing and diagnostics — never a Core that
refuses to start.

**Adapters are honest.** An adapter that cannot do something returns
`ProviderError::Unsupported` with a reason; it never emulates, never guesses, and
never partially performs an operation it cannot complete.

## Consequences

**Good**

- Adding Fedora is one `PackageProvider` implementation plus a verification pass,
  not a sweep through the codebase.
- The UI gets `unavailable` and `degraded` states for free, driven by data rather
  than by a hard-coded list of what works where (`UX-STATES.md`).
- Testing gains a seam: fake providers make the whole API testable without a real
  host, which is how the acceptance criteria in `ROADMAP.md` can run in CI.
- The product stops being able to lie about what a machine can do.

**Costs**

- Interfaces designed against one adapter tend to encode that adapter's model.
  `ContainerProvider` written only for Docker will leak Docker semantics into the
  app platform, and Podman will be harder than it should be. Mitigation: write the
  container spec against the OCI-level concepts both share, and review the trait
  when the second adapter arrives.
- Six traits is more structure than Alpha strictly needs, and the temptation will
  be to bypass them for "just this one call".
- Capability data is another contract to version and another thing a client can
  misread.

**Neutral**

- Some domains will have a thin adapter for a long time. `NetworkProvider` reading
  netlink is nearly the same on every Linux. That is fine; the seam costs little
  and the alternative is discovering the need for it under deadline.

## Alternatives considered

**Direct calls with `cfg!` and runtime OS checks.** Simplest for Alpha. Rejected:
it is precisely the debt the prototype's per-service modules demonstrate, and it
makes the capability model impossible to keep honest.

**A plugin system with dynamically loaded adapters.** Rejected: an enormous
security surface (loading native code into the process that talks to the container
runtime) for a benefit the product does not need — the set of supported OSes is
small and curated.

**Shelling out to a compatibility layer.** Rejected by ADR-004.

**Deriving capability from a support matrix shipped with the release.** Rejected:
it is wrong on the first machine with unusual hardware, and it makes the product
confidently incorrect rather than honestly uncertain.
