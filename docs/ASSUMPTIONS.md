# Assumptions

Every assumption the target design rests on, with a stable identifier. Other
documents cite these as `A-nn`. An assumption that is disproved is a design
change, not a bug — it is recorded here so the change is traceable.

Status values: **held** (believed true, not yet verified against a real
deployment), **verified** (checked on a supported machine), **at risk** (known
counter-examples exist).

## Environment

| ID | Assumption | Status | If wrong |
| --- | --- | --- | --- |
| A-01 | The server is a single machine with a persistent OS install, not a live image or an immutable appliance. | held | Immutable distributions (Fedora CoreOS, NixOS) need a separate packaging path; they were never in the support list. |
| A-02 | The server is on continuously or on a predictable schedule, and is reachable on a private network segment the client also sits on. | held | Discovery and LAN-only trust need a fallback; remote access moves earlier in the roadmap. |
| A-03 | The supported server targets ship systemd as PID 1 and are managed with `apt`. | verified for Debian 12/13 and Ubuntu 24.04/26.04 | Only the adapter changes; the provider interfaces exist precisely for this. |
| A-04 | A container runtime is either present or installable from the distribution's own repositories without third-party sources. | held | The installer must bundle or fetch a runtime, which changes the supply-chain story in [`SECURITY.md`](SECURITY.md). |
| A-05 | The machine has at least 2 GB RAM, 2 CPU threads, x86-64 or aarch64, and 16 GB of free disk for the platform itself. | held | Publish the real floor after Alpha measurements; the number is a claim, not a measurement. |
| A-06 | The server's address may change. Its hostname may collide. Neither is identity. | verified by design | — |
| A-07 | The LAN is not trusted. Other devices on it may be hostile or compromised. | held | This is a design premise, not a prediction; it does not get relaxed. |
| A-08 | mDNS works on the majority of home networks, but not all — some access points block or proxy multicast. | at risk | Manual address entry stays a first-class path, never a hidden fallback. |

## Product

| ID | Assumption | Status | If wrong |
| --- | --- | --- | --- |
| A-10 | The owner will accept exactly one terminal command to install, and zero afterwards. | held | A pre-built installer image or a graphical bootstrapper becomes necessary earlier than planned. |
| A-11 | The owner has administrative access to the server (root or sudo) at install time. | held | There is no product without it; unprivileged installs are not a supported mode. |
| A-12 | Most target users have exactly one server and one to three client devices. | held | Multi-server client UX moves from "supported but plain" to a designed feature. |
| A-13 | Containers are an acceptable delivery mechanism for the application catalogue, including for media applications needing hardware transcoding. | held | Hardware passthrough is the likeliest failure point; see A-14. |
| A-14 | GPU/QSV transcoding can eventually be exposed to a container through device mapping. **Not in Alpha** — ADR-014 refuses device mapping outright, so Alpha transcodes on the CPU. | at risk, deferred | Hardware acceleration becomes a console-granted capability with a documented unsupported state, never a manifest field Core can assert. |
| A-15 | A first-party, curated catalogue of **roughly ten applications at Beta** is enough. Alpha ships one. No third-party publishing. | held | If ten is too few to be useful, the constraint is curation effort, not design; the number moves, the marketplace does not appear. |
| A-16 | Existing containers on the machine belong to the user, not to Atrium, and are shown read-only until explicitly adopted. Enforced by Agent's own ownership evidence, not by Core's word (ADR-013). | held | Adoption semantics need designing earlier. |

## Technical

| ID | Assumption | Status | If wrong |
| --- | --- | --- | --- |
| A-20 | A single self-contained Rust binary per component covers the supported targets. **Linkage is a packaging choice, not architecture** (ADR-007): distribution packages link against the system glibc so NSS keeps working; a static musl tarball exists for unusual systems, without NSS. | held | Only the packaging matrix changes. |
| A-21 | The privileged operation set is closed and enumerable. Every legitimate product feature can be expressed as a typed operation rather than a command. | held | The design fails; a narrow, audited, allowlisted command runner would have to be introduced deliberately, never accidentally. |
| A-22 | Core can read system metrics natively (`/proc`, `/sys`, `statvfs`, netlink) without Glances or any external agent. | verified by prior art | — |
| A-23 | SMART, filesystem and block-device data can be read through existing system interfaces without shelling out to unvalidated tooling, or through a typed Agent operation wrapping a known tool. | held | SMART slips out of the roadmap position given to it. |
| A-24 | TLS with a pinned public key established at pairing time is acceptable to browsers for the web client when the client is a native app, and requires a documented compromise in a plain browser. | at risk | The browser path may require either a local CA the user installs, or an HTTP-on-loopback-only interface with the native apps carrying the pin. Decided in ADR-003; revisit before Beta. |
| A-25 | An append-only local audit log is sufficient for Alpha. Remote log shipping is not needed. | held | — |
| A-26 | SQLite is sufficient as Core's state store for the product's lifetime at this scale. | held | — |
| A-27 | The existing Tauri desktop client can be reduced to an API client without a rewrite of its shell, tabs, appearance and settings surfaces. | held | The client becomes a larger work item than the roadmap allows. |
| A-28 | This project can hold two offline publisher keys — release signing and catalogue publishing — separately from every server's device identity key, and ship the catalogue public key inside the Agent binary (ADR-016). | held | If the keys end up stored or used together, the separation is decorative. If they cannot be held offline at all, a console-registered trust root is the fallback, and the property in `SECURITY.md` §4 weakens accordingly. |
| A-29 | Atrium and the prototype can run on the same machine without colliding, given the reserved identifiers in ADR-015. | held, tested by the canary | The canary plan changes to a separate machine, and Alpha loses its only real-world coexistence test. |
| A-31 | A browser client can be given an honest trust anchor before it needs to pair - a locally installed Atrium CA or an ACME certificate (A-24). Browser pairing is gated on it. | at risk | The `web-pki-v1` binding profile is not honest, and the fallback is a real PAKE (SPAKE2+, RFC 9383) implemented in WASM, which is a significant web-client cost recorded in ADR-003 section 4a. |
| A-30 | The owner will copy or scan a 26-symbol (16-byte, 128-bit) pairing secret rather than retype a short PIN. | held | Short codes need a reviewed PAKE (SPAKE2+, RFC 9383); ADR-003 records that as a prerequisite, not a preference. |

## Deliberately not assumed

- That the user has a domain name, a static IP, or a router they can configure.
- That the user can or will open a port to the internet.
- That the machine has more than one disk.
- That the machine has a display or keyboard attached at any point after install.
- That the clock is correct before first boot, or that NTP is reachable.
- That the catalogue's upstream images will remain available at a given tag.
- That a container runtime's isolation is sufficient to contain a hostile image.
- That the project owns, or will own, any DNS name (ADR-011).
