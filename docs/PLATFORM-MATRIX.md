# Platform support matrix

Status: target policy. Nothing below is claimed to be tested until it is marked
**verified**, which requires a passing installation on real hardware or a VM in
CI, not a plausible code path.

## Support tiers

| Tier | Meaning | Commitment |
| --- | --- | --- |
| **Supported** | Tested every release. Bugs block a release. | Installer, upgrade path, documentation, CI coverage. |
| **Planned** | Intended, adapter work identified, not started or not finished. | Listed with the release it is aimed at. No promises of dates. |
| **Experimental** | Works for someone, may break, no installer guarantees, no upgrade guarantee. | Documented caveats, best-effort fixes, may be withdrawn. |
| **Unsupported** | Will not be made to work, or not now. | Stated reason. Contributions do not change the tier by themselves. |

A tier is a promise about testing and breakage, not about whether something
happens to run.

---

## Server operating systems

The server is where Core and Agent run.

### Supported — Alpha target

| OS | Architecture | Init | Package | Container runtime | Status |
| --- | --- | --- | --- | --- | --- |
| Debian 12 (bookworm) | x86-64 | systemd | apt | Docker CE | Alpha target, not yet verified |
| Debian 13 (trixie) | x86-64 | systemd | apt | Docker CE | Alpha target, not yet verified |
| Ubuntu Server 24.04 LTS | x86-64 | systemd | apt | Docker CE | Alpha target, not yet verified |
| Ubuntu Server 26.04 LTS | x86-64 | systemd | apt | Docker CE | Alpha target, not yet verified |

aarch64 builds of the same four are **planned for Alpha 0.2** — the code is
architecture-neutral, but a Raspberry Pi class device has enough differences in
storage, thermals and transcoding that it earns its own verification pass.

### Planned

| Target | Tier reason | Aimed at |
| --- | --- | --- |
| Debian/Ubuntu on aarch64 | Same adapters, separate verification | Alpha 0.2 |
| Fedora Server (current − 1) | `PackageProvider → dnf`; systemd unchanged; SELinux labelling is real work | Beta |
| Rocky Linux 9/10, AlmaLinux 9/10 | Same adapter as Fedora, different release cadence | Beta |
| Podman (rootful) as `ContainerProvider` | Adapter exists in the design; the app platform must stop assuming Docker semantics | Beta |
| Podman (rootless) | The security end-state. It no longer protects Core, which has no runtime access at all (ADR-013); it shrinks **Agent's** blast radius and the consequence of a container escape | V1 |
| Raspberry Pi OS (64-bit) | Debian-derived; SD-card wear and thermal reporting need care | Beta |

### Experimental

| Target | Why it is only experimental |
| --- | --- |
| Other systemd + apt derivatives (Linux Mint, Pop!_OS, Armbian) | Likely to work. Not tested, not released against. |
| Debian testing/unstable | Moving target. |
| LXC containers as a host | Works if the container can reach the container runtime and read host metrics; many of those reads are wrong or missing inside LXC, and Atrium will report degraded capability rather than pretend. |
| Proxmox VE host | It is Debian, so it installs. Running a control plane next to another control plane is the user's decision, not a supported configuration. |

### Unsupported

| Target | Reason |
| --- | --- |
| TrueNAS SCALE | It is an appliance with its own middleware and app system. Atrium would fight it. Revisit only if a genuine integration path exists, not as a distribution target. |
| Unraid | Same reason, plus a non-standard storage layer that Atrium must not touch. |
| Windows Server | No systemd, no equivalent container/storage story at this scope. `ServiceProvider` and `PackageProvider` would need full Windows implementations. Not before V1, and only if there is demand. |
| macOS as a server | No path to the privileged model described in [`SECURITY.md`](SECURITY.md). |
| Immutable/atomic distributions (CoreOS, Silverblue, NixOS) | The installer's assumptions (A-01) do not hold. A separate packaging model would be required. |
| Non-systemd init (OpenRC, runit, s6) | `ServiceProvider` can express them; nothing else about the product is ready. Not now. |
| 32-bit architectures | Not built. |
| Kubernetes as a runtime | Permanent non-goal — see [`PRODUCT.md`](PRODUCT.md#non-goals). |

---

## Client platforms

A client is a view onto Core's API. It holds a device token and nothing else of
value.

### Supported — Alpha target

| Client | Status | Notes |
| --- | --- | --- |
| Web (Chromium 120+, Firefox 120+, Safari 17+) | Alpha target | Served by Core. The reference client — everything else is an optimisation of this. Self-signed-certificate warning is an open issue (A-24). |
| Windows 10 22H2 / 11 desktop | Existing prototype, to be re-pointed at Core | WebView2, Credential Manager, tray, desktop cards, startup integration all already exist. Code signing is a **public-Beta release gate**; Alpha builds are unsigned and say so. |

### Planned

| Client | Aimed at | Notes |
| --- | --- | --- |
| Linux desktop (Tauri, x86-64 + aarch64) | Alpha 0.2 | Needs Secret Service for the device token and a packaging decision (AppImage vs .deb vs flatpak). |
| macOS desktop (Tauri, Apple Silicon + Intel) | Beta | Needs Keychain integration and notarisation. Apple Developer enrolment is a **public-Beta release gate**, not an Alpha task; nothing needs buying now. |
| Android | V1 | Keystore-backed token, biometric confirmation for destructive operations. |
| iOS / iPadOS | after V1 | Requires a macOS build environment. |
| Android TV | after V1 | A leanback view of media and system state, not the full management surface. |
| Apple TV | after V1 | Same. |

### Experimental

| Client | Why |
| --- | --- |
| Older Safari / older Chromium | May work; not tested, not fixed. |
| Mobile browsers | The web client is responsive but the management surface is designed for a pointer; a mobile browser gets a usable, not a designed, experience until the native apps exist. |

### Unsupported

| Client | Reason |
| --- | --- |
| Internet Explorer, legacy Edge | No. |
| Windows 8.1 and earlier | WebView2 and the toolchain do not target it. |
| Terminal / TUI client | Not a product goal. `atriumctl` exists for recovery only, not as a management interface. |

---

## Capability support, independent of OS

Some features depend on hardware or configuration, not on the distribution. The
UI learns these at runtime from `GET /api/v1/system/capabilities` rather than
assuming them, so this table describes what the *providers* aim to offer.

| Capability | Tier | Notes |
| --- | --- | --- |
| CPU / memory / load / uptime | Supported (Alpha) | procfs. Works everywhere. |
| Filesystem capacity and mounts | Supported (Alpha) | `statvfs` + mountinfo. |
| Network interfaces and link state | Supported (Alpha) | netlink/sysfs. Multiple NICs expected, not an edge case. |
| Container inventory (read-only, incl. the user's own) | Supported (Alpha) | Through Agent. Unmanaged containers are never mutated. |
| Lifecycle of Atrium's own applications | Supported (Alpha) | Through Agent, gated on its ownership registry. |
| Lifecycle of containers the user created | Unsupported | Not a gap: ADR-013 has no operation that can do it. Adoption is a future, owner-initiated workflow. |
| Application install from manifest | Supported (Alpha) | One catalogue app proves the pipeline. |
| Block devices and partitions | Planned (0.2) | sysfs. |
| SMART attributes and health | Planned (0.2) | Needs a typed Agent operation; USB bridges and NVMe differ (A-23). |
| Temperature sensors | Planned (0.2) | hwmon coverage is uneven by hardware; degraded is a normal outcome. |
| Host package installation | Planned (0.2) | No Agent operation exists in Alpha; capability reports `not_enabled_in_alpha`. |
| Shared folders (SMB) | Planned (0.2) | Samba via `PackageProvider` + `ServiceProvider`, configured from validated structures rather than a config file Core supplies. |
| Hardware transcoding passthrough | Unsupported in Alpha, experimental (Beta) | Device mapping is refused outright by ADR-014, so Alpha transcodes on the CPU. It returns behind a console-issued capability grant. The riskiest assumption in the product (A-14). |
| Wake-on-LAN | Planned (Beta) | Inherited from the prototype's deferred list. |
| UPS / battery status | Experimental | Only where NUT or a kernel interface exists. |
| ZFS / btrfs pool reporting | Planned (V1), read-only | Administration remains a non-goal. |
| RAID administration | Unsupported | Permanent non-goal for the foreseeable product. |
| Disk encryption management | Unsupported | Atrium reports whether a filesystem is encrypted; it does not manage keys. |

---

## What a tier change requires

To move from Planned to Supported:

1. The adapters exist and report honest capabilities.
2. A clean install, a pairing, an app install, a reboot and an upgrade all pass on
   that target, in CI or on real hardware, recorded.
3. The installer handles that target's package manager and init without manual
   steps.
4. Its failure modes are represented in [`UX-STATES.md`](UX-STATES.md).
5. It is listed in the release notes as newly supported.

To move anything to Unsupported: a line in this table with the reason, and a
release note. Silently dropping support is not permitted.
