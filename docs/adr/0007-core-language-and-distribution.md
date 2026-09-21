# ADR-007 — Rust for Core and Agent, shipped as self-contained binaries

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-001
**Assumptions:** A-05, A-20

## Context

Core and Agent are new components. The repository already contains 17,000 lines of
Rust (the desktop backend, with its HTTP clients, strict parsers, validation and
provider-shaped modules) and 1,000 lines of standard-library-only Python (the
server agent, written that way so it would run on any distribution's stock
interpreter with no pip).

The server targets are four Debian/Ubuntu releases now and a wider set later, on
x86-64 and aarch64, on machines that may have 2 GB of RAM. Installation has to be
one command. Nothing may depend on a runtime the distribution ships at a version
the project does not control.

## Decision

### Frozen

**Core and Agent are written in Rust**, as two separate binaries.

**Each ships as a self-contained artefact**: no interpreter, no virtualenv, no
runtime to install, no asset directory beside it. `x86_64` and `aarch64` are both
first-class targets.

### Not frozen — a packaging choice, revisited per target

The first revision of this ADR froze *static musl* binaries. Review downgraded
that: it is a build-configuration decision, not an architectural one, and freezing
it buys nothing while constraining several things that matter.

Specifically, static linking against musl costs:

- **NSS.** A statically linked glibc binary cannot load NSS modules at all, and
  musl does not implement the NSS module system. That removes `nss-mdns`, LDAP,
  SSSD and anything else the administrator has configured in
  `/etc/nsswitch.conf`. Core resolving a `.local` name, or an administrator
  expecting their own name resolution to apply, both break silently.
- **Behavioural differences** in DNS resolution, locale handling and stack sizing
  between musl and glibc, on the four targets that are actually supported and
  where glibc is what everything else on the machine uses.

Against that, static musl buys portability to distributions Atrium does not
support. That is a poor trade for the Alpha target list.

Therefore:

| Channel | Linkage |
| --- | --- |
| `.deb` for Debian/Ubuntu (primary) | dynamically linked against the distribution's glibc, with the distribution's minimum version declared |
| `.rpm` when Fedora/Rocky/Alma land | the same, against theirs |
| Generic signed tarball | static musl, offered for unsupported or unusual systems, documented as not covering NSS-based name resolution |

The installer verifies the release signature before unpacking anything, whichever
channel it came from.

**Name resolution and mDNS do not depend on NSS either way.** Core advertises
`_atrium._tcp` with its own mDNS responder rather than requiring Avahi, and
clients resolve discovered servers in-process rather than through the operating
system's `.local` resolution. This keeps discovery working on a machine with no
Avahi, and it keeps the static build honest about what it cannot do. Where Avahi
is present, Atrium coexists with it rather than competing for port 5353 —
registering through Avahi when it is running is a supported alternative path.

**systemd integration needs no library.** `sd_notify` is a datagram on a socket
named by `$NOTIFY_SOCKET`, implemented directly, so neither linkage choice affects
supervision, socket activation or watchdog support.

**The web client is embedded in the Core binary.** One file to install, one file
to upgrade, no asset directory to get out of sync with the running version.

**`atriumctl` is a recovery and bootstrap tool, not a management interface.** It
issues pairing secrets, prints diagnostics, and performs the console-only recovery
operations in `SECURITY.md` §19. It is not the way anyone is expected to use the
product.

**The Python agent is not extended.** It is superseded by Agent (ADR-010).

## Consequences

**Good**

- No runtime dependency. A self-contained binary removes an entire class of
  installation failure, without having to pick one linkage strategy for every
  target the product will ever have.
- The provider layer, the strict parsers and the validation patterns from the
  existing Rust backend port directly rather than being rewritten in another
  language.
- Memory safety in the component that parses untrusted network input while
  holding the keys to the machine.
- Resource use fits the product's floor (A-05). A Python or Node Core would not
  comfortably meet "under 150 MB RSS on a 2015-era dual-core".
- Agent stays small and reviewable, in the same language and toolchain as Core,
  with shared types for the operation enum.

**Costs**

- Rust is slower to write than Python, and the difference is largest in exactly
  the exploratory work this stage involves.
- Two linkage strategies means two build configurations to keep working and a
  matrix that grows with every new target, rather than one artefact that goes
  everywhere.
- The generic musl tarball is a second-class artefact with a documented behaviour
  difference (no NSS). It has to be labelled as such wherever it is offered, or it
  becomes a source of "works on mine" bug reports.
- Writing an mDNS responder rather than depending on Avahi is real work, and port
  5353 coexistence with a running Avahi needs testing on each target.
- Cross-compilation for aarch64 needs CI work that does not exist yet.
- `cargo` builds on Windows are already the slowest CI job; adding two Linux
  crates grows the matrix.
- Some system interfaces have better-maintained Python bindings; a few will need
  FFI or careful `/sys` parsing.

**Neutral**

- The desktop client stays TypeScript + Rust (Tauri). The frontend and Core share
  no code, only the API contract — which is correct, since any client must be
  writable without the platform's source.

## Alternatives considered

**Python for Core**, continuing the agent's language. Rejected: the stdlib-only
constraint that made the agent auditable does not survive a platform Core (HTTP
server, TLS, SQLite, container API, mDNS), and abandoning it means shipping a
virtualenv or depending on distribution Python versions — exactly the installation
fragility this decision avoids. Memory and startup cost are also real on a 2 GB
machine.

**Go.** A genuinely strong fit: fast builds, easy static binaries, first-class
cross-compilation, good libraries for everything here. Rejected on repository
coherence rather than technical grounds — introducing a third language into a
project with an existing large Rust investment costs more than Go's advantages
return, and the code most worth porting is already Rust.

**Node/TypeScript for Core**, sharing types with the web client. Rejected: a
runtime dependency, a large dependency tree in the component that holds the keys
to the machine, and the worst memory profile of the candidates.

**Keep everything in the desktop client and skip Core.** Rejected by ADR-001.

**Distribution via a container.** Rejected: Agent manages the container runtime and
needs host-level access to storage, network and systemd. Running the manager
inside the thing it manages inverts the dependency and complicates every recovery
path.
