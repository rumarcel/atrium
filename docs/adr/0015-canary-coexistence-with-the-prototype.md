# ADR-015 — Alpha runs as a canary beside the prototype

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-010, ADR-011, ADR-013

## Context

The owner has one server, and it is in use. It runs the prototype's Python agent,
twelve services the household actually depends on, and containers created by hand.
Alpha 0.1 is unproven software that will be installed on that same machine.

Two things follow. Alpha must not be able to break what is already there, and the
owner must be able to abandon Alpha at any point without having lost anything.
"Install it and see" is only an acceptable plan if failure is reversible.

This is a product decision recorded as an ADR because it constrains identifiers —
ports, unit names, users, paths — which are expensive to change once anything is
installed anywhere.

## Decision

**M1 and the rest of Alpha install alongside the prototype, not over it. There is
no cutover until Alpha acceptance is complete.**

### Reserved identifiers

Atrium's own namespace, chosen so that nothing collides with the prototype or with
the services already on that machine:

| Kind | Atrium | Prototype it must not touch |
| --- | --- | --- |
| TCP port | `7443` (Core API and web client) | `9473` (Python agent); `8096 7878 8989 6767 8080 8443 9443 9090 49494 8081 3001 61208` (user's services) |
| systemd units | `atrium-core.service`, `atrium-agent.service`, `atrium-agent.socket` | `personal-hub-agent.service`, `personal-hub-inventory@.service`, `personal-hub-inventory@.timer` |
| System user/group | `atrium` | `personalhub-agent` |
| Configuration | `/etc/atrium/` | `/etc/personal-hub-agent/`, `/etc/personal-hub-inventory/` |
| State | `/var/lib/atrium/`, `/var/lib/atrium-agent/` | `/var/lib/personal-hub-agent/`, `/var/lib/personal-hub-inventory/` |
| Program files | `/usr/lib/atrium/` | `/opt/personal-hub-agent/` |
| Runtime | `/run/atrium/agent.sock` | none |
| sudoers | **none** — Agent needs no sudo (ADR-004) | `/etc/sudoers.d/personal-hub-agent` |
| mDNS service | `_atrium._tcp` | none — the prototype does not advertise |
| Container names | `atrium-app-<appId>` | anything else on the machine |
| Container labels | `io.atrium.*` | anything else |
| Container networks | `atrium-<appId>` | anything else |

### Rules

1. **The installer touches nothing outside Atrium's namespace.** It does not
   modify, stop, disable, reconfigure or remove the prototype's units, user,
   sudoers file or state. It does not change the firewall. It does not install a
   container runtime silently.
2. **Port allocation avoids what is already bound.** Before assigning a host port
   to an installed app, Atrium enumerates listening sockets and refuses ports in
   use, plus a compiled reserve list containing `9473` and the prototype's service
   ports. A conflict is reported to the user with the option to choose another —
   never resolved by taking the port.
3. **The prototype's containers are untouchable by construction.** They are
   unmanaged, and ADR-013 gives Agent no operation that mutates an unmanaged
   container. This rule needs no extra mechanism; it is the ownership boundary
   already doing its job.
4. **Uninstalling Atrium restores the machine to its pre-Alpha state**, apart from
   application data the owner chooses to keep. It removes only what the installer
   created.
5. **Both can run indefinitely.** There is no timer, nag or forced migration. The
   prototype's desktop client keeps working against the prototype's agent while
   Alpha is being evaluated.
6. **Cutover happens only after Alpha acceptance**, as a deliberate step: the
   prototype's agent is stopped and removed by its documented uninstall, and the
   desktop client is re-pointed at Core (ADR-009). Nothing about that step is
   automatic.

### What the canary is for

M1 acceptance is measured on a clean VM, because that is where "clean install"
means something. The canary is the *second* environment: a real machine, with real
services, real containers, a real network, and a real owner who will notice if
something breaks. It proves coexistence, which a clean VM cannot.

## Consequences

**Good**

- Failure is reversible, which makes it safe to install Alpha early and often.
- Coexistence is tested from day one rather than discovered at Beta by the first
  self-hoster who installs it (persona P2 in `PRODUCT.md`).
- Identifier collisions are decided once, in writing, before anything is
  installed anywhere.
- The ownership boundary gets an immediate, real-world test: a machine with
  containers Atrium must not touch.

**Costs**

- Two agents run on the same machine during the Alpha period, with two
  privileged installations to keep straight. The prototype's sudoers file
  continues to exist for as long as its agent does.
- Duplicated function — both can report system state — which is confusing to look
  at and tempting to reconcile prematurely.
- The reserve list is a snapshot of one machine's ports. It will be incomplete on
  someone else's; rule 2's live enumeration is what actually protects them, and
  the list is only a belt-and-braces addition.
- Port `7443` is a choice, not a registered assignment. If it collides on some
  machine it must be configurable from `core.toml` — and it is.

## Alternatives considered

**Replace the prototype's agent at M1.** Rejected: it makes the first install of
unproven software a destructive act on a machine in daily use, and there is no
reason to take that risk before acceptance.

**Test only in a VM until Beta.** Rejected: a VM cannot prove coexistence, and
coexistence with software the user already runs is a stated product principle, not
a nice-to-have.

**Reuse the prototype's port and unit names to ease migration later.** Rejected:
it guarantees a conflict during exactly the period when both are installed, and
saves nothing — the migration is a user-visible step either way (ADR-011).
