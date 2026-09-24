# ADR-017 — Identity material is created at installation and owned by root

**Status:** accepted
**Date:** 2026-09-24
**Supersedes:** ADR-003 §1 (the generation and file-mode sentences only),
ADR-012's `secrets.key` mode, and ADR-016's *where it lives* cell for the
server device identity. The rest of each ADR stands.
**Related:** ADR-001, ADR-003, ADR-012, ADR-016
**Evidence:** `server/crates/atriumctl/tests/privileged.rs` (M1B), run against
the real Linux kernel as root and as the real `atrium` user in CI

## Context

Three accepted ADRs describe the server's identity material as something the
running service generates for itself and owns:

| ADR | What it says |
| --- | --- |
| ADR-003 §1 | "At first start Core generates `server_id` … stored in `/etc/atrium/identity.json` (`0600`)", and "Core also generates a TLS key pair" |
| ADR-012 | `secrets.key` at `/etc/atrium/secrets.key` (`0600`) |
| ADR-016 | server device identity "generated per server at first boot; private half at `/etc/atrium/tls.key`, `0600 atrium:atrium`" |

Read together, they describe a service that creates its own key on first start
and holds it in files it owns. A process that owns a file can do anything to it:
rewrite it, truncate it, unlink it, rename another file over it, or chmod it.
That makes arbitrary code execution as `atrium` enough to destroy or replace the
server's identity. The identity would be gone, and every paired client would see
either a broken pin or, worse, a key the attacker chose.

M1 planning found this before any code was written: plan §23, the finding marked
*[fixed, then strengthened]*. The fix went into the plan, into ARCHITECTURE and
SECURITY, and into criterion 6 (amended; ROADMAP *Criteria amended during
planning*). The three ADRs were not superseded at the time. M1B built and proved
the corrected design, and until now the ADR set still said the opposite.

This ADR records the decision that was built. It does not change what was
built.

## Decision

### 1. Final ownership

| Path | Owner | Mode | Meaning |
| --- | --- | --- | --- |
| `/etc/atrium/` | `root:atrium` | `0750` | the `atrium` group may traverse and list; nobody but root may create, unlink or rename entries |
| `/etc/atrium/tls.key` | `root:atrium` | `0640` | the device identity private key; Core reads it to serve TLS |
| `/etc/atrium/identity.json` | `root:atrium` | `0640` | `server_id` and the identity record |
| `/etc/atrium/secrets.key` | `root:atrium` | `0640` | the key for the M3 secrets database, created now and first used in M3 |
| `/etc/atrium/core.toml` | `root:atrium` | `0640` | bootstrap configuration |

> **Note (M1E, 2026-09-24).** `secrets.key` is first used in M1E, not M3:
> [ADR-019](0019-sealed-pairing-secret.md) derives from it the key that seals
> the armed pairing secret. Its ownership, mode and creation are unchanged.

The group bits allow read, and there is no group write bit anywhere in the
directory. The guarantee rests on **discretionary access control**, which the
kernel enforces whatever the unit file says. `ReadOnlyPaths=/etc/atrium` in
`atrium-core.service` is defence in depth, not the mechanism.

The running Core, as `atrium`, **can read** each file and **cannot** write,
truncate, unlink, rename over, chmod or chown it, and cannot create a file,
directory, symlink or hard link beside it. Core also checks these ownerships and
modes at every start, as properties rather than one exact mode: root-owned,
writable by nobody else, the files not readable by "other". A tree where the
guarantee visibly fails starts in recovery mode with `identity.unprotected`.
Core does not run on it.

### 2. Created once, at installation, by an explicit step

The identity is created by `atrium-core init-identity`, a separate short-lived
invocation that the installer runs **once**, while `/etc/atrium/` is still
`atrium:atrium 0700`. It writes `tls.key`, `secrets.key`, the state database at
schema 1, the certificate, and last `identity.json`, all at `0600` owned by
`atrium`. The installer then hands the files to `root:atrium 0640` and the
directory to `root:atrium 0750`, verifies the result, and fails the install if
anything differs (plan §5.1, §12.2 step 8).

The long-running service **never generates identity**. If the files are absent
when Core starts, it enters recovery with `identity.missing`. If they are
partial, it enters recovery with `identity.inconsistent`. It never regenerates:
a running Core that can mint an identity is a running Core that can destroy one.

`atrium-core init-identity` refuses a finished installation (exit `3`, nothing
changed), refuses a partial or inconsistent tree, and refuses to run as root.

### 3. Rotation is a console action by root

`atriumctl rotate-identity` runs as root at the console. It writes the new key
as `root:atrium 0640` and keeps `server_id`, then drops to `atrium` for good
before it touches the state database. Device revocation is bound to the key
through `settings.identity.spki_sha256`, so the next opener of the database
revokes every device. No process that could regain root ever parses
service-writable data.

### 4. What is superseded, exactly

| ADR | Superseded text | Replaced by |
| --- | --- | --- |
| ADR-003 §1 | "At first start Core generates `server_id`" and "stored in `/etc/atrium/identity.json` (`0600`)"; "Core also generates a TLS key pair" | §2 of this ADR (install-time `init-identity`) and §1 (`root:atrium 0640`) |
| ADR-012, databases table | "`/etc/atrium/secrets.key` (`0600`)" | `root:atrium 0640`, created at installation (§1, §2) |
| ADR-016, decision table | "generated per server at first boot; private half at `/etc/atrium/tls.key`, `0600 atrium:atrium`" | generated per server **at installation**; `root:atrium 0640` in a `root:atrium 0750` directory |

**Not superseded, and explicitly reaffirmed:** `server_id` is 128 random bits
and stable for the life of the installation; the key is ECDSA P-256 and never
leaves the machine; the certificate is reissued from the same key when
addresses change; clients pin `SHA-256(SPKI)`; the pairing construction,
binding profiles and the SHA-256 device-token verifier (ADR-003 §2 onwards);
the SQLite store and forward-only migrations (ADR-012); the separation of the
three trust roots and everything about the release and catalogue keys
(ADR-016). This ADR changes who creates the files and who owns them. It
reopens nothing else.

The superseded ADRs are not rewritten. Each carries a short note pointing here,
so the history of the decision stays readable.

## Consequences

**Good**

- **A Core compromise cannot destroy or replace the server's identity.** It can
  read the key, and that is unavoidable for any process that terminates TLS
  with it. It cannot rotate, truncate, delete or substitute the key, or plant a
  file next to it. This narrows ADR-001's Core-compromise outcome from "the
  attacker's own data and the machine's identity" to "the attacker's own data".
- **The property is proven, not asserted.** M1B's privileged suite makes each
  write attempt as the real `atrium` user against the real kernel: write,
  truncate, unlink, rename-over, rename-away, chmod, chown, create, mkdir,
  symlink and hard link. It asserts every errno. The read is asserted too, so
  the test cannot pass by locking Core out.
- **Missing identity is loud.** Recovery says `identity.missing` instead of
  silently creating a new identity that would strand every paired client.
- **Generation is a reviewed, one-time, install-time act.** It runs in a
  separate process with a separate exit contract, not as a branch inside the
  long-running network service.

**Costs**

- The installer does more, and has to do it correctly: create the directory,
  run `init-identity` as `atrium`, change ownership, verify. The installer
  verifies its own result and fails the install on any difference.
- A machine where someone deletes `/etc/atrium` by hand does not heal itself.
  Recovery reports it, and the fix is a console action. That is deliberate.
- Core must be able to read a root-owned file through its group, so the
  files' group must be `atrium`: Core's primary group, and its only one, since
  the unit sets `SupplementaryGroups=` empty. Core does not check the group
  name. A wrong group shows up as a failed read (`identity.unreadable`) and
  never as a wider grant, because "other" has no access at all.

**Neutral**

- During `init-identity` the files are briefly `0600 atrium:atrium`. That state
  exists only inside the installer's sequence, before any service is started.

## Alternatives considered

**First-boot generation by the service, `0600 atrium:atrium` (the superseded
text).** Rejected: this is the defect. The owner of a file can replace it.

**`root:root 0600` and let Core read through a capability.** Rejected: Core's
capability bounding set is empty by design (ADR-001, SECURITY §9), and
`CAP_DAC_READ_SEARCH` would let Core read every file on the machine.

**Have Agent hold the key and perform TLS operations for Core.** Rejected for
M1: it puts a signing oracle behind the privileged boundary and adds a
per-handshake operation to the Agent protocol, which is a much larger authority
than a read-only file. It is worth revisiting only if key exfiltration by a
compromised Core becomes a threat the product must defend against.
Read-but-not-replace answers the threat this ADR addresses: destruction and
substitution.

**Rely on `ReadOnlyPaths=` alone.** Rejected as the mechanism: it holds only
while the unit file is intact and systemd applies it. It stays as defence in
depth.
