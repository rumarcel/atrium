# Atrium product roadmap

Stages, not dates. A stage is finished when its exit criteria are met, and not
before.

The prototype's own phase history — Phase 1 through 9.2 of the Windows desktop
application — lives in the repository's root [`ROADMAP.md`](../ROADMAP.md). That
document is history and stays accurate; this one is the forward plan for the
platform.

---

## Stage 0 — Prototype preservation

**Goal:** keep the working thing working while the platform is built beside it.

- The Windows desktop application continues to build, test and install exactly as
  it does today. No regressions are introduced by platform work.
- The prototype is documented as a reference implementation rather than as the
  product — see [`PROTOTYPE-ASSESSMENT.md`](PROTOTYPE-ASSESSMENT.md).
- Identifier migration (`personal-hub` → `atrium`) is planned but not executed
  mid-flight; it lands with a data-preserving migration (ADR-011).
- **Done in the hardening pass:** the owner's real LAN address is gone from every
  tracked file — the bundled seed, both systemd units, the agent's install guide,
  the UI placeholders and the test fixtures. RFC 5737 documentation addresses
  could not be used everywhere, because several code paths validate that an
  address is RFC1918 and would reject `192.0.2.x`; the placeholder `10.0.0.10` is
  used where a private literal is required, and range coverage in the tests was
  preserved rather than flattened. The agent's listening address became
  configuration (`ATRIUM_AGENT_HOST` in an `EnvironmentFile`) instead of a
  constant in two unit files.
- Still outstanding, and the real fix: the bundled seed should be an **empty**
  catalogue with a first-run "add your server" step, rather than twelve example
  entries pointing at a host that does not exist. That is a behaviour change to
  the prototype and lands with the client's migration, not in a documentation
  pass.
- Platform work happens in `core/` and `agent/` alongside the existing tree, not
  by rewriting it.

**Exit criteria:** platform development can start without touching the desktop
application's behaviour, and the published repository contains no personal
network detail.

---

## Alpha 0.1 — The spine

**Goal:** prove Core, Agent, pairing, providers and the app platform end to end on
one supported machine, with one application.

Scope is the ten points in [`PRODUCT.md`](PRODUCT.md#alpha-01-scope). In build
order:

1. **M1 — Core skeleton, identity, pairing, system read.** Core and Agent install
   under systemd, survive reboot, generate identity, advertise over mDNS, pair a
   client with a secret, and serve `GET /api/v1/system`, `/system/metrics` and
   `/system/capabilities` natively. No containers. The container *boundary* is
   nonetheless proved structurally here, because everything later is built on it.
   Detailed acceptance criteria in §M1 below.
2. **M2 — Provider layer and containers.** `ContainerProvider` and
   `ServiceProvider` **inside Agent**, the ownership registry, read-only
   inventory including the user's own containers, start/stop/restart for Atrium's
   own apps only, the operation resource and the event stream. Acceptance criteria
   in §M2 below.
3. **M3 — Application platform.** Manifest v1, bundled catalogue with Jellyfin,
   install/uninstall, volumes, ports, secrets, health, persistence across reboot.
4. **M4 — Web client.** The reference client for everything M1–M3 exposes,
   implementing the state set in [`UX-STATES.md`](UX-STATES.md).
5. **M5 — Diagnostics and honesty pass.** Every failure path produces a code, a
   diagnosis and raw detail; the diagnostics bundle exists; the audit log is
   readable.

**Where it runs.** Acceptance is measured on a clean Debian 12 virtual machine,
because that is where "clean install" means anything. In parallel, Alpha is
installed on the owner's real server **beside the prototype, never over it**
(ADR-015) — that is the canary, and it is what proves coexistence with real
services, real containers and a real network. Cutover to Atrium happens only
after acceptance is complete.

**Exit criteria:** a clean Debian 12 machine goes from nothing to Jellyfin playing
a file using one terminal command, with the client discovering and pairing on its
own, and survives a reboot and an IP change without re-pairing. Every failure
injected during testing produces a diagnosable state rather than a spinner. The
canary machine keeps running everything it ran before.

**Not in Alpha 0.1:** users, shares, disks, SMART, backups, updates, remote
access, notifications, downloads, media integrations, game servers, the desktop
client's migration.

### M1 — acceptance criteria

M1 is the recommended first implementation milestone. It is finished when all 48
of these are demonstrably true on a clean Debian 12 virtual machine, with evidence
recorded in CI or in a written test log, **and** criteria 42–45 are demonstrably
true on the owner's server running the prototype. Partial credit does not exist.

**Installation**

1. A single documented command installs Core and Agent, creates the `atrium`
   system user, installs the systemd units, and starts both services. It prints a
   pairing secret and the server's addresses.
2. The installer makes no change to the firewall, no change to any existing
   service, and installs no container runtime without stating it.
3. `systemctl status atrium-core atrium-agent` shows both active after the
   install and again after `reboot`.
4. Uninstall removes everything the installer created, leaves `/var/lib/atrium`
   only if the operator chooses, and leaves no unit files behind.

**Identity and hardening**

5. Core runs as `atrium`, not root. `ps -o user= -p $(pidof atrium-core)` proves
   it. Agent runs as root and has no listening TCP socket
   (`ss -lntp` shows nothing for it).
6. The device identity private key **cannot be replaced by the service
   identity**. `/etc/atrium/tls.key` is `0640 root:atrium` inside a
   `0750 root:root` directory; the identity file and the secrets key are the
   same. Proven two ways: by `stat`, and by attempting — as the `atrium` user —
   to write, truncate, unlink and replace each file and to create a new file in
   the directory, all of which must fail. The agent socket is `0660 root:atrium`.
   (Amended 2026-09-22; see *Criteria amended during planning* below.)
7. `server_id` and the TLS key pair survive a reboot, a Core restart and a Core
   upgrade. Changing the machine's IP address reissues the certificate without
   changing the key pair.
8. Agent rejects a connection from any uid other than Core's, verified with a test
   that connects as another user.

**Discovery and pairing**

9. A client on the same LAN lists the server without being given its address.
10. Manual address entry reaches the same server and produces identical results.
11. Pairing with the correct secret succeeds and returns a device token; the
    client verifies the server's `proofS` **before** storing anything, and stores
    it in the OS keychain.
12. The pairing secret is **exactly 16 bytes from the OS CSPRNG, 128 bits**,
    encoded as **exactly 26 Crockford Base32 symbols**. Tests assert: the
    generator emits 16 bytes; the encoder emits 26 symbols from the Crockford
    alphabet; hyphens and whitespace are stripped before decoding and change
    nothing; uppercasing is ASCII-only, proven by decoding a lowercase secret
    under a Turkish locale; `O`/`I`/`L` map to `0`/`1`/`1`; a 25- or 27-symbol
    input is rejected; a non-canonical final symbol (padding bits non-zero) is
    rejected; the QR payload decodes to the identical 16 bytes; and comparison is
    on the decoded bytes, never the text. A shorter secret is a build failure, not
    a configuration option (ADR-003 §3, §5).
13. Pairing with a wrong secret fails with a single generic `pairing_rejected`,
    and five failures disable pairing until it is re-armed on the server.
14. A used secret cannot be reused. An expired secret cannot be used. A
    `begin`/`complete` pair older than two minutes is rejected.
15. A man-in-the-middle presenting a different TLS key cannot complete pairing
    even when given the correct secret — proven by a test that substitutes the
    SPKI and the TLS exporter in the transcript and requires the server to reject.
    The transcript's **binding profile** is part of what is proven: a proof
    computed under a different profile does not verify, and a client asking for a
    profile the armed secret does not permit is refused. (Amended 2026-09-22.)
16. A server that cannot produce a valid `proofS` is rejected by the client, which
    reports `untrusted` and stores nothing — proven by a test with a server that
    returns a well-formed but wrong `proofS`.
17. The first pairing claims the server and creates the owner. A second pairing
    attempt against a claimed server without a fresh secret is refused.
18. After the server's IP address changes, the already-paired client reconnects
    with no re-pairing and no warning.
19. Rotating the server's identity key breaks every pin, forces re-pairing, and
    presents that as a deliberate action rather than as an attack.
20. Revoking a device from another client invalidates its token on the next
    request, within one request, not on expiry.

**API and data**

21. `GET /api/v1/system` returns OS, kernel, architecture, hostname, uptime and
    boot id, read natively. No Glances, no external agent, no third-party
    monitoring dependency anywhere in the path.
22. `GET /api/v1/system/metrics` returns CPU, memory and load. Values match
    `/proc` within tolerance, verified against a known load.
23. `GET /api/v1/network/interfaces` returns every interface and every address on
    a two-NIC machine. Nothing in the code path assumes a single address.
24. `GET /api/v1/storage/filesystems` returns real capacity for every real mount,
    with pseudo and bind mounts excluded.
25. `GET /api/v1/system/capabilities` reports each provider's availability, and
    reports a *missing* feature with a reason code on a machine that lacks it —
    verified by testing on a VM with no container runtime installed.
26. Every unauthenticated request to anything outside the pairing surface returns
    `401` with a problem object; no data leaks in the error.
27. Every response carries `X-Atrium-Api: 1` and `X-Atrium-Version`.
28. Unknown fields in a request body are rejected, not ignored.

**Failure and diagnostics**

29. Stopping Agent makes privileged capability unavailable with reason
    `agent_unreachable`, while every read endpoint keeps working.
30. Corrupting the state database starts Core in recovery mode. It does not
    crash-loop; `/healthz` reports the recovery state and the reason; redacted
    diagnostics remain available; the corruption is clearly reported rather than
    masked. **Recovery mode exposes no state-changing route of any kind** — no
    restore, no pairing, no device change, no Agent call — proven by sweeping the
    router. Restore is performed locally with `atriumctl restore`, after which
    normal Core resumes and previously paired devices still work.
    (Amended 2026-09-22; see *Criteria amended during planning* below.)
31. Every error response carries a stable `code`, a `diagnosis` and a
    `requestId`. A test enumerates the code table and fails if any route can
    produce an uncoded error.
32. `GET /api/v1/system/diagnostics` names the selected providers and their
    versions. The diagnostics bundle contains no secret, verified by a test that
    scans the bundle for known secret values.
33. Every privileged operation appears in Core's audit log and, independently, in
    Agent's journal.

**Container boundary — structural, provable without containers**

M1 builds no container features. These criteria prove the *boundary* exists before
anything is built on top of it, which is the whole point of doing them at M1.

34. `id -nG atrium` lists no supplementary groups at all, and specifically not
    `docker`. The Core unit sets `SupplementaryGroups=` empty, and a test asserts
    the running process's group list.
35. Core cannot open the container runtime socket. A test runs, as the `atrium`
    user, an explicit `connect()` to `/var/run/docker.sock` on a machine where
    Docker is installed, and requires `EACCES` — not "no client library present",
    which would pass vacuously.
36. There is no code path in Core that links, imports or vendors a container
    runtime client. A dependency-level test fails the build if one appears.
37. No HTTP route proxies, forwards or wraps a runtime API. A test enumerates
    every route and fails on any handler that takes a container identifier or an
    opaque body destined for a runtime.
38. The Agent operation table contains no variant whose parameter is a container
    id, an arbitrary unit name, an arbitrary package name, an arbitrary path, an
    opaque byte payload, **a public key, a key identifier, a key path, or a flag
    that skips or relaxes signature verification**. The test enumerates the enum
    and fails on any unvalidated `String`, `PathBuf` or `Vec<u8>` parameter, and
    on any parameter whose type could carry trust material (ADR-016).
39. With Docker installed but no Atrium apps, `GET /api/v1/system/capabilities`
    reports the container capability through Agent (`"via": "agent"`) — proving
    the read path already goes through the boundary rather than around it.
40. With Agent stopped, the container capability reports `available: false` with
    reason `agent_unreachable`, and no Core code path attempts an alternative.
41. `/var/lib/atrium-agent/` is `0700 root:root` and unreadable by the `atrium`
    user, verified by attempting to read it as that user.

**Canary coexistence on the owner's server**

42. Installing Atrium on a machine already running the prototype's agent changes
    nothing about it: `personal-hub-agent.service` is still active, its port 9473
    still bound by it, its user, sudoers file and state directory untouched.
    Verified by capturing the machine's unit list, listening sockets and
    `/etc/sudoers.d/` before and after, and diffing.
43. Atrium binds only `7443`, and nothing else. `ss -lntp` shows no other
    Atrium-owned listener.
44. No file outside `/etc/atrium/`, `/var/lib/atrium/`, `/var/lib/atrium-agent/`,
    `/usr/lib/atrium/`, `/run/atrium/` and Atrium's own unit files is created or
    modified by the installer. Verified by a filesystem diff of the install.
45. Uninstalling Atrium returns the machine to its pre-install state apart from
    data the operator chooses to keep, and the prototype still works afterwards.

**Non-negotiables, verified by test**

46. There is no route, in any build, that accepts a command, argv, shell
    fragment, arbitrary unit name, arbitrary package name or arbitrary host path.
    A test enumerates the Agent operation table and fails on any free-form string
    parameter that is not validated against a closed set.
47. No component calls a shell. A grep-level test fails the build on
    `sh -c`, `bash -c` or an unvalidated `Command` invocation outside the
    allowlisted call sites.
48. Core makes no outbound network connection during normal operation other than
    an explicitly configured update check, verified by running with the internet
    disconnected for the full acceptance run.

### Criteria amended during planning

Two criteria were rewritten during M1 planning and one was extended. The rule is
that a criterion may be amended only by making the property it protects stronger,
never by making it easier to pass, and the change is recorded rather than applied
quietly.

**Criterion 6 — identity key ownership.**

- *Was:* "`/etc/atrium/tls.key`, the identity file and the secrets key are `0600`
  and owned by `atrium`."
- *Now:* `0640 root:atrium` inside a `0750 root:root` directory, proven by `stat`
  **and** by failed write, truncate, unlink, replace and create attempts as the
  `atrium` user.
- *Why this is stronger:* the old wording made the key owned by the service
  identity, which meant arbitrary code execution as `atrium` could replace the
  server's identity — forcing every paired device to re-pair and changing what the
  server *is*. The new wording makes that impossible at the discretionary-access
  layer, not merely at the systemd layer, and it is the only version of the
  criterion that can be proven by attempting the attack.

**Criterion 30 — catastrophic recovery.**

- *Was:* "starts Core in recovery mode: it serves `/healthz`, diagnostics and the
  restore path, and does not crash-loop."
- *Now:* the same no-crash-loop, health, diagnostics and clear-reporting
  requirements, plus **no state-changing route in recovery mode at all**, with
  restore performed locally by `atriumctl restore` and normal service resuming
  afterwards.
- *Why this is stronger:* a remote restore endpoint that works when the state
  database is unreadable needs an authentication source outside that database. The
  two ways to build it are an unauthenticated remote restore — which lets a LAN
  attacker roll the server back to an older state — or a duplicate authentication
  store, which duplicates the most security-sensitive state in the product and can
  disagree with it exactly when a revocation lands mid-corruption. Removing the
  remote restore removes both. The cost is honest and stated: catastrophic
  recovery needs the console, which is a deliberate exception to the product's
  no-terminal goal, documented in `SECURITY.md` §19 rather than hidden.

**Criterion 15 — pairing binding.** Extended to cover the transcript's binding
profile, because the profile is what keeps the future browser client from
requiring a different protocol. Nothing was removed.

### M2 — container boundary acceptance criteria

Recorded here so they are not lost, and because they are the behavioural half of
the property M1 proves structurally. **They belong to M2, not M1** — M1 does not
build containers.

1. Installing a catalogue app produces a container named `atrium-app-<id>`,
   labelled `io.atrium.managed=true` and `io.atrium.app=<id>`, with an owner token
   recorded in Agent's registry and returned by no API response anywhere. A test
   greps every API response body and the diagnostics bundle for the token value
   and fails if it appears.
2. `AppStart`/`AppStop`/`AppRestart`/`AppRemove` succeed for that app.
3. A container created outside Atrium (`docker run`) appears in the inventory
   marked unmanaged, and **no** API call or Agent operation can start, stop,
   restart or remove it. The test attempts each one and requires
   `container.not_managed`.
4. A container created outside Atrium that has been hand-labelled
   `io.atrium.managed=true` and `io.atrium.app=jellyfin` is still refused, because
   it has no registry entry and no matching owner token. This is the test that
   proves labels alone are not evidence.
5. Deleting Agent's registry entry for a managed app causes the next mutation to
   be refused with `container.not_managed` — Agent never falls back to labels.
6. Altering a managed container's `io.atrium.owner-token` causes the next mutation
   to be refused with `container.ownership_mismatch`, the app to become
   `disowned`, and Agent to journal it independently of Core.
7. Recreating a container with the same name but a different id causes
   `container.ownership_mismatch` rather than silent inheritance.
8. A manifest requesting `privileged`, a host path, a device, host networking or
   an added capability is refused with `agent.spec_refused` at both catalogue
   validation and `AppApply`.
9. An `AppApply` carrying a manifest whose signature does not verify against
   **Agent's own key** is refused, even when Core reports the signature as valid.
   The test substitutes a Core that lies.
10. An `AppApply` whose accompanying specification differs from what Agent derives
    from the manifest is applied **as Agent derived it**, and the discrepancy is
    journaled. The test sends a specification with an extra bind mount.
11. Port allocation refuses a port already bound, including `9473` and the
    prototype's service ports, and reports the conflict rather than taking it.
12. A catalogue whose `index.json.sig` does not verify against Agent's compiled-in
    publisher key is refused with `agent.catalogue_signature_invalid`, and no API
    call, setting or operation parameter can override it.
13. A manifest whose bytes do not hash to the digest recorded in the verified
    index is refused with `agent.manifest_digest_mismatch`, including when Core
    reports the manifest as valid.
14. The publisher trust root is the same after a Core compromise simulation as
    before it: a test drives every operation in the table and asserts Agent's
    accepted key set is byte-identical afterwards.
15. Installing a key into `/etc/atrium/agent/publisher-keys.d/`, or enabling
    unsigned local manifests, makes the capability surface report
    `catalogueTrust: "extended"`, and the interface shows it. A test asserts the
    reduced-trust state cannot be entered from the API.

---

## Alpha 0.2 — Making it a server

**Goal:** the things a person actually installs a home server for.

- Shared folders over SMB, with permissions, through `PackageProvider` +
  `ServiceProvider`.
- Disks, partitions, filesystem detail and SMART health, with honest `degraded`
  reporting where hardware does not cooperate.
- Storage capacity planning: what is using space, per app and per share.
- The desktop client re-pointed at Core, keeping its shell, tabs, appearance,
  cards and Windows integration; local service catalog imported as external links.
- Linux desktop client build.
- aarch64 server support, verified.
- Catalogue grown towards its Beta target of roughly ten curated applications —
  media, downloads, photos, documents.
- Notifications, re-pointed at Core's event stream.

**Exit criteria:** an owner can store, share and play their files, and manage the
machine from Windows or Linux, without a terminal.

---

## Beta — Trustworthy

**Goal:** it can be recommended to someone who is not going to read the source.

- Users, roles and household accounts.
- Backups: scheduled, verified, restorable, with app quiescing from the manifest.
- Update centre for the platform and for apps, staged with automatic rollback.
- Remote access: the owner's VPN documented, managed WireGuard built.
- The browser certificate question resolved (A-24).
- Fedora / Rocky / Alma support via `dnf`; Podman rootful as a container provider.
- macOS client.
- A real recovery story tested by actually breaking things: corrupt database,
  failed migration, lost devices, half-finished install, full disk.
- Security review of the API surface, the pairing exchange and the Agent
  operation table.

**Exit criteria:** every recovery path in [`SECURITY.md`](SECURITY.md#19-recovery-and-revocation)
has been executed on a real machine and works.

---

## V1 — A product

- Rootless containers as the default, shrinking Agent's blast radius and the
  consequence of a container escape. Core never had runtime access to give up.
- Hardware transcoding as a supported, probed capability rather than an
  experiment.
- Android client with biometric confirmation for destructive operations.
- App upgrades with settings migration and permission-change consent, exercised
  across a real breaking upstream change.
- Storage reporting for ZFS and btrfs, read-only.
- Documentation and onboarding good enough that the interface is the manual.
- A published support policy: which server OS versions, for how long.

**Exit criteria:** the three success criteria in
[`PRODUCT.md`](PRODUCT.md#success-criteria) are met, and a first-time owner
reaches a working media server without asking anyone anything.

---

## Later — platform expansion

Ordered by conviction, not by date. Each needs its own design pass.

- iOS, Android TV and Apple TV clients.
- Game server management as a catalogue category with its own manifest extensions.
- Multiple servers in one client, as a list of independently paired machines —
  never a cluster.
- Photo, document and sync applications as first-class catalogue citizens.
- Windows as a *server* target, if there is demand and someone to maintain it.
- A pinned remote catalogue, still curated, still not a marketplace.

## Permanently out

Kubernetes. RAID and ZFS administration. A public third-party marketplace. An AI
assistant. A generic remote shell. Mandatory cloud. Telemetry.
