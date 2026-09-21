# ADR-001 — Core and Agent separation

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-004, ADR-007, ADR-009, ADR-013

## Context

Today the Windows desktop application is the whole product: it holds
configuration, credentials, HTTP clients and all integration logic, and it talks
directly to third-party services on the server. When the window closes, nothing
of Atrium is running.

A platform has to keep running. Applications must start after a reboot, health
must be observed continuously, backups must run when nobody is watching, and a
second client must see the same state as the first. None of that is possible from
a client process.

The server side must also do things that need root — install packages, manage
system services, read SMART data. Running the whole server component as root
means a bug anywhere in HTTP parsing, JSON handling, container spec building or
the web UI is a root compromise of the machine.

## Decision

Two server-side components:

**Atrium Core** — runs as the unprivileged `atrium` user under systemd. Owns the
API, the state store, identity, pairing, sessions, roles, audit, the provider
layer, the application lifecycle, scheduling and the bundled web client. Core is
the only component clients talk to.

**Atrium Agent** — runs as root under systemd, socket-activated on
`/run/atrium/agent.sock` (`0660 root:atrium`). Has no network listener, no
configuration surface and no routing. Executes a closed set of typed operations
submitted by Core, verifies the peer uid with `SO_PEERCRED`, and keeps its own
append-only journal.

Rules that follow:

- Core never runs as root, never calls `sudo`, and never executes a shell.
- Core holds **no container-runtime access of any kind** — not the `docker` group,
  not the socket, not a proxy. The runtime belongs to Agent (ADR-013).
- Agent never accepts a command, argv, shell fragment, arbitrary unit name,
  container id, opaque byte payload, or a path outside a compiled allowlist
  (ADR-004).
- Clients never reach Agent, directly or by proxy.
- A read that needs privilege is either collected by Agent on a schedule into
  Core's store, or reported as an unavailable capability. Reads do not cross into
  root on demand.

## Consequences

**Good**

- A remote-code-execution bug in Core yields the `atrium` user and the closed
  operation set, not the machine. The operation set is the product's own feature
  list, and every use of it is logged by a component the attacker does not
  control.
- Agent is small enough to review completely. Its attack surface is one Unix
  socket and one enum.
- The platform runs with no client connected, which is the entire point.
- Hardening directives that the prototype's sudo-based agent could not use
  (`NoNewPrivileges=true` among them) become available, because Agent is already
  root and needs no elevation.

**Costs**

- Two components to install, supervise, version, upgrade and keep compatible.
  Their protocol needs its own versioning and a mismatch needs a defined
  behaviour.
- Every new privileged capability is a deliberate addition to the operation
  table, reviewed as a security change. This is friction by design and it will
  feel slow.
- Agent is larger than it would be if it only did power and service operations,
  because ADR-013 puts the container runtime behind it. Agent's size is itself a
  security property, so this cost is paid knowingly and re-examined whenever
  something new is proposed for it.

**Neutral**

- The Python power agent is superseded. Its safety patterns are carried forward
  (ADR-010); its code is not.

## Alternatives considered

**Single root server process.** Simplest, and how most self-hosting control
panels are built. Rejected: it makes every parsing bug a root bug, and the product
explicitly targets non-expert owners who cannot assess that risk.

**Core as root with privilege dropping per request.** Rejected: the privileged
code path is then the same process image as the HTTP parser, and correctness
depends on never making a mistake in the drop.

**Keep the sudoers model from the prototype.** Rejected: it forces
`NoNewPrivileges=false`, blocks several hardening directives, requires an
administrator-reviewed sudoers file per capability, and does not scale past two
operations.

**Client-only, no server component.** Rejected: the product cannot exist.
