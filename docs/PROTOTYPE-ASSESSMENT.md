# Prototype assessment

What the repository contains today, what is worth keeping, and what blocks
productization. Written from an inspection of the tree at commit `aee5a72`.

This is a technical record, not a criticism. The prototype does what it set out to
do and does it carefully; the gaps below are gaps between *that* goal and the
platform goal, which are different products.

## 1. What this repository actually is

A **Windows desktop client** for a home server the user already built. It is not a
server platform and was never intended to be one.

```
Windows desktop (Tauri v2)                         Linux server
┌──────────────────────────────────┐              ┌──────────────────────┐
│ React 19 + TypeScript UI         │              │ Glances (3rd party)  │
│  ├─ dashboard, tabs, settings    │──HTTP───────▶│ Homarr (3rd party)   │
│  └─ appearance, i18n, widgets    │              │ qBittorrent (3rd p.) │
│                                  │              │ 12 other web UIs     │
│ Rust backend (17k LOC, 17 files) │              │                      │
│  ├─ all HTTP clients             │──HTTPS:9473─▶│ personal_hub_agent   │
│  ├─ credential vault (WinCred)   │   bearer +   │  (Python, 608 LOC)   │
│  ├─ WebView2 child views         │   pinned cert│  reboot / shutdown   │
│  └─ trust policy, validation     │              │  read container snap │
└──────────────────────────────────┘              └──────────────────────┘
```

Everything the product knows lives in the desktop process. Close the window and
nothing remains except three third-party services the user installed themselves.

Sizes: 17,112 lines of Rust across 17 modules; ~100 TypeScript/TSX files under
`src/`; 1,011 lines of Python in `server-agent/`; 9 frontend test files, Rust unit
tests inline, two Python test modules; CI covering all three.

## 2. Findings

### 2.1 There is no server-side component to build on

The Python agent (`server-agent/personal_hub_agent.py`) is the only thing that
runs on the server, and it is deliberately tiny: two power actions and a read of
container snapshot files written by a separate root timer. It has no state store,
no user model, no application concept, no capability model.

Everything else — metrics, discovery, downloads, health, credentials — is client
code talking to third-party services. **The Core described in
[`ARCHITECTURE.md`](ARCHITECTURE.md) does not exist in any form.** This is the
single largest gap and it is structural, not incremental.

### 2.2 The data model is links, not applications

`public/config/services.json` is a list of `{ id, name, url, icon, category,
enabled, tlsPolicy, authentication }`. A "service" is a bookmark with a health
check and an optional credential. Atrium can open it, check it, and log into it —
but it did not install it, cannot start it, does not know its version, owns none
of its data and cannot remove it.

The platform's `App` is a different entity entirely. The link model does not
extend to it; it survives alongside it, as `GET /api/v1/links`.

### 2.3 Third-party services are load-bearing

- **Glances** is the only source of CPU, memory, storage, network, load and
  uptime. No Glances, no dashboard data — the code correctly reports
  "not configured" rather than inventing values, but the capability is gone.
- **Homarr** is the only service-discovery source besides the agent's container
  snapshot.
- **qBittorrent** is the Download Center's only provider.

Each has a dedicated Rust module with its own response parsing, its own credential
adapter and its own validation state machine. This is the prototype's main
technical debt for productization: **integrations are code, not data.** It is the
exact pattern [`APP-SDK.md`](APP-SDK.md) exists to replace.

### 2.4 Identity is an IP address

`server_control.rs` accepts only a bare RFC1918, loopback or link-local IPv4
literal as the enrolled address. The port is fixed at 9473. The bearer token is
stored in Windows Credential Manager under
`PersonalHub/server-control/v1/<ip>:9473`, so the credential is *keyed by the
address*. Changing the server's address requires a full re-enrollment with a
re-entered token.

This is a sound decision for a single-user tool on a static-IP LAN and an
impossible one for a product. It directly violates the product rule that an IP is
not identity, and it is why ADR-003 introduces `server_id` plus SPKI pinning.

### 2.5 The prototype's security work is genuinely good and should be reused

This is the most valuable finding. The patterns below are already implemented,
tested, and correct, and the platform inherits them rather than reinventing them:

- **No shell, ever.** `dispatch_power` uses a fixed argv tuple, `shell=False`, a
  fixed `PATH`, closed stdio and a timeout. There is no code path anywhere that
  builds a command from input.
- **No Docker socket exposure.** Container inventory is a file written by a
  separate root-only timer running one fixed read-only command; the network-facing
  agent has no socket access, no Docker group and no CLI.
- **Durable intent before privileged action.** The operation is committed to the
  journal *before* the power command is dispatched, and a lost response stays
  `executing` until a new boot id is observed. A never-confirmed operation is
  never retried.
- **Strict parsing in both directions.** Unknown fields rejected, duplicate JSON
  keys rejected, bounded bodies and headers, closed enums for every state and
  reason, no upstream error text ever returned to the frontend.
- **Origin-bound credentials.** Every stored secret is bound to a normalized
  scheme/host/port; a credential whose service URL changed is visible as stored
  but never transmitted.
- **Host header allowlisting and `Origin` rejection** in the agent — DNS-rebinding
  and browser-origin defence already present.
- **Capability-scoped IPC.** Only the trusted `main` WebView receives Tauri
  capabilities; remote `service-*` WebViews get none, and each privileged command
  re-verifies the caller label.
- **The frontend never supplies a URL.** Rust resolves every target from the
  trusted catalog by id. This is the ancestor of the API rule in
  [`API.md`](API.md#8-what-will-never-be-added-to-this-api).
- **Honest unavailability.** No fabricated uptime, no local Windows values
  substituted for server values, no polling of an endpoint that cannot exist.

### 2.6 Privilege model needs replacing, for a good reason

The agent runs unprivileged and escalates through two exact sudoers entries
(`/usr/bin/systemctl reboot`, `/usr/bin/systemctl poweroff`). Because `sudo` needs
privilege elevation, the systemd unit is forced to `NoNewPrivileges=false` and the
unit file carries a long comment explaining which hardening directives cannot be
used because they imply it.

The platform's split — an already-root, socket-activated Agent with
`NoNewPrivileges=true`, `PrivateNetwork=true` and no `sudo` at all — is strictly
stronger and removes the sudoers file entirely (ADR-004).

### 2.7 Windows coupling

`src-tauri/Cargo.toml` puts `auto-launch`, `webview2-com`, the `windows` crate and
three Tauri plugins behind `cfg(windows)`. `credential_vault.rs` calls `CredWriteW`
directly. `service_webviews.rs` (2,212 lines) is built on WebView2's child-view
model, certificate-error callbacks and native HTTP Basic handling.

The coupling is honest and contained, but it means "cross-platform client" is real
work, not a build flag. The correct reading is that these modules are *desktop
client* code, and the platform logic that must be cross-platform is the code that
moves to Core anyway.

### 2.8 Installation requires a terminal and an expert

`server-agent/README.md` is 374 lines of `useradd`, `install -o root -m 0640`,
`openssl req -x509 -addext 'subjectAltName=IP:...'`, fingerprint verification,
`visudo -cf`, and a manual dry-run validation before real power is enabled.

Every step of that is justified for what it is. None of it is compatible with
"normal product usage should not require terminal commands". The platform's
installer must do all of it in one command, and the manual path becomes the
expert's escape hatch, not the only path.

### 2.9 Discovery does not exist at the network level

There is no mDNS, no SSDP, no scanning. "Discovery" means *asking Homarr for its
app list* or *reading the agent's container snapshot*. The user must already know
the server's address to configure anything. Alpha needs real discovery plus
manual entry (A-08).

### 2.10 Naming and identifier debt

Three namespaces deliberately kept as `personal-hub` because changing them
destroys user data:

| Identifier | Value | Consequence of changing |
| --- | --- | --- |
| Tauri identifier | `com.muhta.personal-hub` | the per-user config directory moves; settings appear lost |
| Credential targets | `PersonalHub/credentials/v1`, `PersonalHub/server-control/v1/<ip>` | stored secrets become unreachable |
| Agent units/account | `personal-hub-agent.service`, `personalhub-agent` | server-side reinstall |

Also: crate name `personal-hub`, lib name `personal_hub_lib`, package name
`personal-hub`, event names `personal-hub://…`, user agent
`PersonalHub/0.1 service-discovery`, thread name `personal-hub-close-to-tray`, CI
artefact `personal-hub-windows-unsigned`.

None of this is urgent; all of it is confusing in a public repository named
`atrium`. ADR-011 defines a migration that preserves data.

### 2.11 Publishing hygiene — found, and fixed in the hardening pass

The repository shipped the author's real LAN address — a literal this document
deliberately does not repeat — in 21 tracked files: the bundled seed catalogue's
twelve entries, both systemd unit
files, the agent's installation guide, the English and Turkish UI catalogs, two
UI placeholder values, seven Rust test modules and four JavaScript test modules.
Not exploitable — the range is not routable — but a real address in a public
repository, and it hard-coded a topology the product must not assume.

It is now gone. Two details worth recording, because they are the kind of thing a
blind find-and-replace gets wrong:

- **RFC 5737 documentation addresses could not be used everywhere.** `192.0.2.x`
  is not RFC1918, and several code paths — the enrolled-address validator, the
  agent's `validate_host`, the local-TLS exception — require a private literal and
  correctly reject it. Those places use `10.0.0.10`, which is a placeholder on
  nobody's network but still passes the validation being tested. RFC 5737
  addresses remain in use where they always were: as the *rejected* examples in
  those same tests.
- **Range coverage was preserved, not flattened.** The tests that exercise
  `192.168.0.0/16` specifically kept a `192.168.x` literal (`192.168.10.20`)
  rather than collapsing onto `10.0.0.10` and quietly losing a branch.

The agent's listening address was also the wrong kind of thing to replace: it was
hard-coded in two systemd units, which is configuration masquerading as source. It
is now `ATRIUM_AGENT_HOST`, read from `/etc/personal-hub-agent/agent.env` through
`EnvironmentFile=` with no leading `-`, so a missing or empty value fails the unit
instead of falling back to a guess.

Two things were found and deliberately **kept**: `rumarcel` in `LICENSE` and
`Cargo.toml` is the author's chosen public handle, which is attribution, not
leakage; and `com.muhta.personal-hub` is a data-bearing identifier whose change
moves every user's configuration directory, so it stays until ADR-011's migration
runs. No email addresses, absolute personal filesystem paths, tokens, passwords,
API keys or private hostnames were found anywhere in tracked files.

### 2.12 What is in good shape and needs no work

- **CI.** Three jobs — frontend types and tests on Linux, Rust build/fmt/clippy/
  test on Windows, Python agent tests on Linux — with pinned toolchains and a
  correctly keyed cargo cache. Extends naturally to Core and Agent jobs.
- **Test discipline.** Contract tests with fake clocks, temporary journals and
  mocked dispatch; the Python tests never issue a power command.
- **Documentation.** The README is unusually precise about what the software
  refuses to do. The ROADMAP records product decisions, not just features. Both
  set the register this document set tries to match.
- **Appearance and i18n.** A versioned theme-token contract with schema-validated
  packs that reject URLs, markup, scripts and remote fonts; typed translation
  catalogs with a key-parity test. Reusable as-is.
- **Frontend structure.** Feature-scoped modules with a `*.types.ts` /
  `*Client.ts` / `components/` convention and strict TypeScript. The client shell
  is worth keeping.

## 3. Prototype limitations that matter for the platform

Ordered by how much they constrain the next step.

1. **No server-side runtime.** Everything must be built. Not a refactor.
2. **Integrations are code, not manifests.** The pattern must be replaced before
   the catalogue grows past a handful of entries, or the debt compounds per app.
3. **IP-as-identity.** Blocks pairing, blocks DHCP, blocks multi-homed servers.
4. **No install story.** The server side is a documented manual procedure.
5. **Metrics depend on a third party.** Alpha needs native collection.
6. **Read-only by design.** Container inventory, downloads and monitoring are all
   deliberately non-mutating. The platform must introduce mutation as a
   first-class, audited, recoverable concept — the power-control path is the only
   existing example and it is the model.
7. **Single user, single server, single client, Windows only.** Four assumptions
   baked into the data model and the storage layer.
8. **No API.** There is Tauri IPC, which is not a network contract and cannot be
   consumed by a web or mobile client.

## 4. What the platform inherits, explicitly

| From the prototype | Into the platform |
| --- | --- |
| Fixed-argv, no-shell privileged dispatch | Agent operation execution |
| Durable intent before privileged action; `unknown` outcomes never retried | Operation resource semantics |
| Strict parsing, closed enums, bounded I/O | API contract and provider layer |
| Origin-bound credentials, `{ kind, exists }` responses | Secret store and API rules |
| Host allowlisting, `Origin` rejection | Core's HTTP layer |
| Capability-scoped IPC, caller verification | Role checks and client boundaries |
| Honest unavailability, no invented values | [`UX-STATES.md`](UX-STATES.md) |
| Bounded, sanitized container inventory — no image repository, environment, labels or command line | Agent's `ContainerInventory` operation |
| Never exposing a container socket to anything that talks to the network | ADR-013, which goes further: not even to Core |
| Atomic writes, backup-before-replace, recovery notices | Core's state store and migrations |
| Desktop shell, tabs, WebView profiles, appearance, i18n, tray, cards | Desktop client, re-pointed at Core |
| CI structure and test discipline | Extended, not replaced |
