# Atrium — product definition

Status: target definition for the productization pass. Nothing in this document
is implemented yet unless [`PROTOTYPE-ASSESSMENT.md`](PROTOTYPE-ASSESSMENT.md)
says the prototype already covers it.

## Vision

**Turn any computer into your personal cloud.**

A person already owns a machine that is switched on most of the time — an old
laptop, a mini PC, a tower that was replaced last year, a self-built NAS. Atrium
turns that machine into a personal server with a real interface: storage that is
shared properly, media that plays on the television, downloads that finish
without a terminal, applications that install in one step and keep working after
a reboot.

The comparison points are UGREEN UGOS Pro and Synology DSM. The difference is
that those ship with the hardware. Atrium does not care what the hardware is.

## What Atrium is

A platform, in three parts:

- **Atrium Core** — a service installed on the server. It owns the machine's
  state, exposes one authenticated API, and is the only component that knows
  what distribution, init system or container runtime is underneath.
- **Atrium Agent** — a small privileged helper next to Core. It performs a fixed
  set of typed privileged operations and nothing else.
- **Atrium clients** — the web interface served by Core, plus native desktop
  applications. A client is a view onto the API. It holds no privilege of its
  own.

Applications are installed from declarative manifests rather than from
service-specific code inside the interface. Adding Jellyfin to the catalog must
not require a Jellyfin feature branch in the UI.

## Users

**P1 — The owner-operator (primary).** Owns the machine and the data on it.
Comfortable installing an operating system from a guide; not a system
administrator. Wants media, files, downloads and backups to work, and wants to
understand what broke when something breaks. Will not maintain YAML, reverse
proxies or certificate renewals.

**P2 — The self-hoster (primary).** Already runs Docker on this machine, already
has compose files and opinions. Will adopt Atrium only if it coexists with what
is already there, never fights their terminal, and never silently rewrites their
containers. Judges the product on whether the abstraction leaks safely.

**P3 — The household (secondary, after Alpha).** Uses the services; does not
administer the server. Needs accounts, permissions and an interface that does not
present destructive controls.

**Anti-persona.** The operator of a fleet. Multi-node clustering, tenant
isolation, Kubernetes and datacentre storage administration are out of scope and
stay out of scope; designing for them would compromise every decision that makes
the product approachable.

## Principles

1. **Hardware agnostic.** No assumption about vendor, CPU architecture beyond a
   declared support list, disk layout, NIC count or the presence of a GPU.
2. **Coexistence.** Atrium is installed onto a machine that already has an owner,
   an OS, and possibly other software. It never assumes exclusive ownership of
   the host, never reformats, and never manages resources it did not create
   without explicit consent.
3. **Local-first.** Every core capability works with the internet unplugged. Cloud
   connectivity is optional, opt-in, and never required for LAN operation.
4. **Privacy-first.** No telemetry, no account, no phone-home, no analytics
   endpoint. Data leaves the machine only when the owner directs it to.
5. **Beginner friendly, expert capable.** The default path needs no terminal. The
   expert path exposes the raw underlying detail — exit codes, logs, the actual
   command — rather than hiding it behind a friendly message.
6. **Secure by default.** The safe configuration is the default configuration.
   Anything that widens exposure is an explicit, reversible, audited choice.
7. **Recoverable.** Every operation that can fail has a defined outcome when it
   does. Configuration is backed up and restorable. A bad update can be rolled
   back. A lost client cannot lock the owner out of their own machine.
8. **Modular.** Capability is discovered and negotiated at runtime, not assumed
   from a distribution name.
9. **Honest.** The interface never invents a value it does not have, never
   reports success it has not observed, and never claims a guarantee it cannot
   keep. This principle is inherited from the prototype and is not negotiable.

## Product boundaries

Atrium **is**:

- a control plane for one machine, paired with one or more client devices
- an application platform built on containers
- a storage, sharing, networking and health surface for that machine
- the owner of the applications it installs, including their data and lifecycle

Atrium **is not**:

- an operating system, a distribution, or an imaging/installer tool
- a hypervisor or a VM manager
- a router, firewall or DNS server for the network
- a cloud storage service or a backup destination
- a replacement for the applications it installs
- a multi-machine orchestrator

Atrium **must not**:

- require terminal commands for normal use after installation
- expose arbitrary command execution, a remote shell, or the Docker TCP socket
- give its unprivileged Core any container-runtime access — no `docker` group, no
  `/var/run/docker.sock`, no TCP socket, no socket proxy, no equivalent by another
  route. The runtime belongs behind the privileged Agent (ADR-013).
- mutate a container the user created, until an explicit adoption workflow exists
- use a LAN IP address as the identity of a server
- hold privileged credentials in the user interface
- require cloud connectivity for LAN operation
- assume one machine, one NIC, one disk or one address forever

## Alpha 0.1 scope

Alpha proves the platform's spine end to end on a clean supported machine. It is
narrow on purpose: one application installed from one catalog entry is enough to
prove the manifest pipeline; two applications are not more proof.

In scope:

1. Install Core and Agent from a single documented command on Debian 12/13 or
   Ubuntu Server 24.04/26.04.
2. Core and Agent start under systemd and survive a reboot.
3. A client on the same LAN discovers the server without being told its address.
4. The client pairs with the server through a code that proves physical or
   administrative access to the machine, and the trust survives an address change.
5. System state: CPU, memory, storage capacity, network interfaces, uptime, load,
   OS identity — read natively by Core, with no third-party monitoring agent.
6. Container inventory: enumerate what the container runtime is running —
   read-only, including containers the user created, which Atrium never mutates.
7. Container lifecycle for **Atrium's own applications**: start, stop and restart
   a managed app, as an audited operation with an observable result.
8. Install one catalog application (Jellyfin) from a manifest, reach its web
   interface, and have it come back after a reboot.
9. Uninstall that application, with an explicit choice about its data.
10. Diagnostics: any failed operation produces a stable error code, a
    human-readable diagnosis, and raw detail available on demand.

Explicitly deferred out of Alpha 0.1 — see [`ROADMAP.md`](ROADMAP.md): shared
folders and SMB, users beyond the single owner, disks and SMART, backups, the
update centre, remote access, notifications, media integrations, download
management, game servers, and the desktop clients beyond a functional build.

## Product decisions recorded in this pass

**Alpha runs as a canary.** M1 installs on the owner's real server *alongside* the
existing prototype, not over it. Ports, unit names, users and state directories are
reserved so the two cannot collide, and the prototype keeps working throughout.
Cutover happens only after Alpha acceptance is complete, as a deliberate step.
Details and the reserved identifier table are in ADR-015.

**The catalogue stays small.** Beta targets roughly **ten** curated first-party
applications. That is a curation commitment, not a technical ceiling, and it is
not Alpha scope — Alpha ships one application, because one proves the pipeline and
two do not. A large public marketplace remains a permanent non-goal.

**Hardware acceleration is deferred to its own security ADR.** Alpha does not
support GPU transcoding, because device passthrough is part of the permission set
Agent refuses outright (ADR-014). It returns — if it returns — through a dedicated
post-Alpha ADR covering console-issued capability grants, not by relaxing the
constraint set or adding a flag.

**Code signing is a release gate, not an implementation task.** Windows code
signing and Apple Developer enrolment plus notarisation are prerequisites for a
*public Beta* on those platforms. Nothing needs to be purchased or implemented now,
and Alpha builds are unsigned and say so — as the prototype's already do.

## Non-goals

Permanent non-goals, restated so they are not rediscovered later:

- Kubernetes, or any multi-node scheduler.
- RAID and ZFS pool administration. Atrium will read and report array health long
  before it is allowed to create or destroy an array.
- A public application marketplace with third-party publishers.
- An AI assistant.
- Cockpit as an API dependency. Cockpit may remain a linkable service; it is not
  a platform layer.
- Generic remote shell, SSH proxying, or a "run this command" endpoint.
- Mandatory accounts or a mandatory relay service.

## Success criteria

**Alpha 0.1 is successful when**, on a clean supported machine, a person who has
never seen the project can go from a fresh OS install to Jellyfin playing a file,
using exactly one terminal command (the installer) and no documentation beyond
the interface itself — and when that machine can be rebooted, given a new IP
address by the router, and still be reachable by the same paired client without
re-pairing.

**Beta is successful when** an existing self-hoster can install Atrium on a
machine that already runs their containers, and Atrium neither breaks them nor
claims them.

**V1 is successful when** the owner can lose the client device entirely, pair a
new one, and recover full control without a terminal, and when a failed update
leaves a working system.

Measurable Alpha targets:

| Target | Threshold |
| --- | --- |
| Install to first paired client | under 10 minutes, one command |
| Core idle memory | under 150 MB RSS |
| Core idle CPU | under 1% of one core on a 2015-era dual-core |
| Dashboard first paint after pairing | under 2 seconds on LAN |
| Survives reboot with no manual step | required |
| Failed operation without a diagnosis | zero tolerated |

## Assumptions

Product and technical assumptions are registered with stable identifiers in
[`ASSUMPTIONS.md`](ASSUMPTIONS.md) and referenced from the other documents as
`A-nn`. Anything in this document that depends on an unverified belief cites one.
