# ADR-004 — Closed typed privileged operation set

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-001, ADR-002, ADR-013, ADR-014, ADR-016
**Assumptions:** A-21

The first revision of this ADR contained an operation, `FileWrite { file:
ManagedFile, content: Vec<u8> }`, that would have let Core hand raw bytes to a
root process for writing to a privileged file. A review found that this is a root
execution primitive in disguise: Samba's `smb.conf` supports `root preexec`, and
most privileged configuration formats have an equivalent. It is removed below and
replaced with structured configuration operations. The same revision moves
container-runtime access behind this boundary (ADR-013).

## Context

Every capability the product wants — installing Samba, enabling a service, reading
SMART, rebooting, running an application — needs privilege. The expedient design is
an endpoint that runs a command, or an Agent whose operation takes a unit name, a
package name, a path, or a blob. Every self-hosting control panel that has been
remotely exploited got there this way.

The prototype already refuses the obvious version of this: its power dispatch is a
fixed argv tuple with `shell=False`, a fixed `PATH` and closed stdio, and its
sudoers file lists two exact commands with no wildcards. That is the right instinct
and it needs to become a rule that survives feature pressure — including the
less obvious versions, like a byte payload or a container id.

## Decision

**Privileged work is expressed as a closed enum of typed operations, compiled into
Agent.** There is no operation whose parameter is a command, argv, script, shell
fragment, opaque byte payload, container identifier, or free-form string that
reaches a process invocation or a privileged file.

```rust
// Illustrative shape. Every variant's parameters are validated types, not strings.
enum PrivilegedOp {
    // --- Atrium's own services only -------------------------------------
    PlatformServiceStart   { unit: AtriumUnitName },   // io.atrium namespace only
    PlatformServiceStop    { unit: AtriumUnitName },
    PlatformServiceRestart { unit: AtriumUnitName },
    PlatformServiceEnable  { unit: AtriumUnitName },
    PlatformServiceDisable { unit: AtriumUnitName },
    PowerAction            { action: PowerAction },    // Reboot | Shutdown

    // --- Containers: domain intent, never a container id (ADR-013) -------
    AppApply    { manifest: ManifestBytes, indexEntry: CatalogIndexEntry,
                  indexSignature: Ed25519Signature,      // data to verify, never a key
                  settings: ValidatedSettings,
                  allocations: Allocations, secrets: SecretBundle },
    AppStart    { app: AppId },
    AppStop     { app: AppId, timeout: Duration },
    AppRestart  { app: AppId },
    AppRemove   { app: AppId, volumes: VolumeDisposition },
    AppInspect  { app: AppId },
    AppLogs     { app: AppId, tail: LogLines, since: Option<Timestamp> },
    ContainerInventory,                                 // read-only, sanitized, all containers
    RuntimeInfo,                                        // runtime name, version, capabilities

    // --- Storage (0.2) ---------------------------------------------------
    SmartRead   { device: EnumeratedBlockDevice },      // device ids Agent itself enumerated
    MountApply  { source: EnumeratedBlockDevice, target: PlatformMountPath,
                  options: MountOptionSet },            // nosuid,nodev forced by Agent

    // --- Shares (0.2): structured, never raw bytes -----------------------
    ShareApply  { shares: Vec<ShareSpec> },             // Agent renders the config itself

    // --- Packages (0.2): allowlisted set only ----------------------------
    PackageEnsure { package: AllowlistedPackage, state: Present | Absent },
}
```

Constraints on every variant:

1. **Validated types, not strings.** `AtriumUnitName` can only be constructed from
   a platform-owned unit. `AppId` refers to an app in Agent's own registry.
   `AllowlistedPackage` is an enum. `EnumeratedBlockDevice` can only be
   constructed from a device Agent itself enumerated in this process.
   `PlatformMountPath` cannot escape the platform's tree. A variant that cannot
   express its parameter as a validated type does not get added.
2. **No opaque payloads to privileged files.** Where Atrium must write privileged
   configuration, the operation carries **validated structures** and Agent renders
   the file itself from a template it owns. A configuration format is usually a
   programming language in disguise; handing it bytes is handing it code.
3. **No container identifiers.** Container mutation takes an `AppId` and Agent
   resolves it through its own ownership registry (ADR-013). Inventory returns ids
   for display; no operation accepts one.
4. **No trust material.** No operation carries a public key, a key identifier, a
   key path, a trust-root file, or a flag that skips or relaxes signature
   verification. `AppApply` hands Agent content to verify; the means of verifying
   it is compiled into Agent and is unreachable from Core (ADR-016).
5. **Fixed absolute executables, fixed argument templates, no shell**, explicit
   environment, timeout, bounded output. The prototype's `dispatch_power` is the
   pattern.
6. **Authenticated at the transport**: `SO_PEERCRED`, Core's uid only. The socket
   is `0660 root:atrium`.
7. **Authorized before it is sent**: Core has already checked the caller's role.
   Agent trusts Core for authorization and for nothing else — not for ownership,
   not for spec contents, not for catalogue authenticity.
8. **Durable intent before dispatch.** The operation is committed to Agent's
   journal before anything privileged runs, so a crash mid-operation is
   reconstructible and a lost result is `unknown`, never silently retried.
9. **Audited twice**: Core's audit log, and Agent's independent journal, so a
   compromised Core cannot erase what it asked for. Refusals are journaled as
   loudly as successes.
10. **Rate limited and serialized** per operation class, by Agent itself. One
    power action at a time; one image pull at a time; a bounded queue.
11. **Bounded and cancellable where physics allows**, with `cancellable: false`
    stated honestly once a point of no return is passed.

**Alpha 0.1 ships a deliberately smaller table than the one above.** Package,
storage and share operations are not implemented; their capabilities report
`available: false` with a reason. The fewer operations exist, the smaller the
authority a compromised Core inherits, and each one is added with its own review.

**Adding an operation is a security change.** It needs a stated need, a validated
parameter type, a test proving the parameter cannot escape its domain, and a line
in the audit vocabulary. Reviewers are expected to push back.

**Permanently prohibited**, in any component, in any build:

- an endpoint or operation that accepts a command, argv, script or shell fragment
- an operation that accepts an arbitrary unit name, package name, container id or
  host path
- an operation that accepts opaque bytes destined for a privileged file
- an operation that carries a public key, a key path, a trust-root file, or a flag
  that skips or relaxes signature verification
- SSH proxying, or anything that behaves like a remote shell
- exposing the container runtime's socket or API to Core or to a client
- passing user input through a shell

## Consequences

**Good**

- The worst case for a Core compromise is the product's own feature set, logged —
  and that claim is now enforceable rather than aspirational
  (`SECURITY.md` §4).
- The operation table is a complete, readable inventory of everything Atrium can
  do to a machine. It can be audited by reading one file.
- Fuzzing and property tests have a small, typed, total surface.
- Agent stays small enough to review completely.

**Costs**

- **Friction, permanently.** Every new privileged feature is a schema change plus
  a review. This will be the most-complained-about decision in the project and the
  one least worth relaxing.
- Rendering privileged configuration from structures means Atrium must model every
  setting it exposes. Samba has hundreds; Atrium will support a handful and say so,
  rather than offering a config box.
- Some legitimate capability is simply harder — anything involving a
  user-specified path or an unlisted package needs real design rather than a
  passthrough.
- A-21 may be wrong for some future feature. If it is, the response is a narrow,
  audited, explicitly-allowlisted mechanism introduced by a superseding ADR with
  its own threat analysis — never an incremental widening of a parameter type.

**Privilege-adjacent operations, named honestly**

Two future operations have effects at root level even though their parameters are
closed, and they are called out so they are never added casually:

- `PackageEnsure` runs the distribution's package manager, which runs maintainer
  scripts as root. The compiled allowlist and the distribution's own package
  signing are the entire security boundary. Not in Alpha.
- `MountApply` changes what is visible at a path. Agent forces `nosuid` and
  `nodev`, restricts targets to the platform's mount tree, and refuses to mount
  over a path that is not empty. Not in Alpha.

## Alternatives considered

**`POST /api/v1/exec` with a role check.** Rejected. It is the vulnerability.

**Allowlisted command strings with pattern matching.** Rejected: allowlists over
strings are defeated by argument injection, and the pattern grows one exception at
a time until it is not an allowlist.

**A generic `FileWrite` with an allowlist of target paths** — the previous
revision. Rejected on review: the path allowlist constrains *where*, and the
attacker only needs *what*. A config file that can spawn a process makes the path
irrelevant.

**Polkit rules.** Reasonable on paper. Rejected: another policy language to get
right, availability varies, and it does not remove the need for typed parameters.

**Keeping sudoers, one entry per operation.** The prototype's model. Rejected: it
forces `NoNewPrivileges=false`, requires an administrator-installed file per
capability, and a wildcard slipped into one line is a full compromise.

**Core runs as root and does it directly.** Rejected by ADR-001.
