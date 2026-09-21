# Atrium roadmap

This roadmap records product direction rather than promising fixed release dates.
The current dark Atrium appearance remains the initial and fallback theme.

> This document is the **Windows desktop application's** phase history and
> remains accurate for it. The forward plan for Atrium as a server platform is in
> [`docs/ROADMAP.md`](docs/ROADMAP.md), and the target architecture it belongs to
> starts at [`docs/README.md`](docs/README.md).

## Product decisions

- Do not add a dashboard clock, calendar or visible polling timestamps. Internal
  timestamps remain available only for scheduling, freshness and stale-data logic.
- Theme changes apply to Atrium's dashboard, navigation and settings. Remote
  service pages run in isolated native WebViews and keep their own appearance.
- Invalid or missing appearance settings always fall back to the bundled default.
- The bundled `Code` theme keeps the same navigation, content and features; it
  changes the visual language to a restrained code-editor aesthetic rather than
  pretending to be a terminal or command-line interface.
- The main dashboard composition stays fixed and calm. Customization is limited
  to the service area, and never changes themes, credentials or native commands.
- The dashboard server summary must show server uptime as a duration. This is not
  a clock, date or visible polling timestamp.
- Desktop cards are experimental, opt-in and disabled by default. Disabling them
  must prevent their windows and polling work from being created, not merely hide
  them after startup.
- Programmatic API authentication and embedded-page login are separate concerns.
  Generic password injection into arbitrary service pages is never allowed.

## Completed foundation

- Phases 1-4: desktop shell, dashboard, validated service catalog and health checks.
- Phase 5: native service tabs.
- Phase 5.1: warm WebView pool and persistent per-service profiles.
- Phase 5.2: service-player fullscreen integration.
- Phase 6: Glances server monitoring.
- Phase 6.1: server-only Windows desktop cards.
- Phase 6.2: dashboard uptime, optional-provider handling and deterministic
  service-media teardown.
- Phase 7.0: writable Settings, validated per-user service configuration and a
  Windows-backed credential vault.
- Phase 7.1: origin-bound provider authentication, native validation and
  exact-origin WebView2 HTTP Basic handling.
- Phase 7.2: persistent background runtime, opt-in desktop cards and native
  tray lifecycle.
- Phase 7.3: persisted appearance, four bundled themes, safe Theme Studio packs
  and Turkish/English localization.
- Phase 7.4: category-based service tabs plus persistent card/logo views.
- Phase 7.5: authenticated Homarr discovery, review-before-add and automatic
  local service icons.
- Phase 7.6: read-only qBittorrent Download Center with safe native sessions.
- Phase 8: Windows integration, optional server notifications and NSIS packaging.
- Phase 9.0/9.1: single-enrolled-address server-control implementation and
  restricted Linux agent; manual enrollment/dry-run validation remain required
  before activation.

## Phase 6.1 — Windows desktop cards (completed)

- Three independent companion surfaces for server metrics, server storage and
  server-service attention, outside the Atrium dashboard.
- At the end of Phase 6.1, the in-app dashboard remained in its Phase 6 layout
  with no widget editor, reordering or card-size controls.
- Borderless transparent windows stay below ordinary applications, skip the
  taskbar and never attach to Explorer/WorkerW internals.
- A single Rust broker shares bounded Glances trends, uptime and load data while
  keeping each card's IPC response scoped to that card.
- Metrics and health polling activate independently and only while a visible
  card needs that data.
- Position and size persist per card, with automatic recovery when saved geometry
  no longer intersects an available monitor.
- Phase 6.1 initially shipped hide-until-restart behavior; Phase 7.2 replaces
  it with persistent per-card Settings and tray controls.
- No local Windows hardware telemetry, clock/date or credential-dependent
  provider integration.

## Phase 6.2 — Stability and media lifecycle (completed)

- Added a clearly visible Glances uptime duration to the dashboard's existing
  `Server online` summary without adding another dashboard widget.
- Closing a service tab destroys its child WebView so Jellyfin and other media
  cannot continue playing invisibly.
- Switching between still-open tabs may keep their warm WebViews and session
  state; closing and switching must have intentionally different semantics.
- A definitively missing or disabled Glances provider produces an explicit
  not-configured state and no continuous monitoring poll. It never produces fake
  uptime or other server telemetry, while independent service health continues.
- Glances-dependent Server and Storage desktop cards are unavailable or omitted
  when that provider is absent; Service attention remains independent.
- The Phase 7.2 tray runtime closes all service child WebViews before hiding the
  main window. Invisible background playback remains off by default.
- Regression coverage protects service-tab close, media teardown, unavailable
  uptime and optional-provider behavior.

## Phase 7 — Settings and integrations

### Phase 7.0 — Settings and credential vault (completed)

- Writable per-user service configuration with validated recovery.
- Service add, edit, remove, enable and disable flows.
- Persistent service preferences with explicit backup, restore and reset paths.
- Windows-backed secret storage for API keys, bearer tokens, HTTP Basic
  credentials and provider-specific usernames/passwords.
- Credentials remain outside repository files, URLs, logs and exported settings.
- Removed-service credentials use a durable, non-secret cleanup journal with
  startup retry, visible recovery state and safe ID reuse blocking.

### Phase 7.1 — Authentication and session providers (completed)

- Provider-specific adapters consume secrets only in Rust; the frontend sees
  credential presence, closed validation states and safe metadata, never a
  stored username or secret value.
- Homarr `ApiKey`, Glances HTTP Basic and Glances bearer adapters are
  allowlisted. Homarr validates against its protected `/api/info` endpoint;
  Glances credentials are applied to version discovery and metric requests.
- Credentials are bound to normalized scheme, host and effective port. Legacy
  entries and credentials saved for a previous endpoint must be replaced before
  they can be transmitted.
- Validation is single-flight, bounded, redirect-free and protected by capped
  retry backoff. Credential or catalog revision changes discard stale results.
- Persistent WebView profiles remain the primary browser-login mechanism.
  WebView2 HTTP Basic is an explicit, exact-origin adapter with a per-view
  submission limit; replacing that credential first destroys the old view.
- No universal DOM form filler or script-based password entry exists. A
  provider-specific OIDC/SSO handoff remains a dormant extension point until a
  supported service exposes a documented flow that needs one.

### Phase 7.2 — Background runtime and experimental desktop cards (completed)

- An `Experimental desktop cards` master switch, disabled by default, plus
  independent Server, Storage and Service-attention card switches.
- Disabled cards create no window, WebView, polling claim or background work.
- Close-to-tray behavior while background features are enabled, with explicit
  `Open Atrium` and `Quit` actions.
- Tray controls to show/hide cards, reset geometry and open Settings.
- Supported `always below normal windows` mode as the default card layer.
- A separate, clearly labeled Explorer desktop-layer experiment may be offered
  only after Windows-version, Explorer-restart, DPI and multi-monitor testing;
  it is never the default.
- Runtime preferences use an exact, versioned schema, cross-process serialized
  revision-checked writes and atomic per-user persistence. The safe
  first-run/recovery state creates no card window or polling claim.
- Card windows are created dynamically only for effective selections. Removing
  Glances closes Server/Storage cards; adding it back restores selected cards.
- Closing the main window first suspends service-view creation, releases every
  child WebView and only then hides the window. A failed teardown keeps the
  main window visible, while tray Open resumes the service lifecycle.
- Card close controls now disable that card persistently. Geometry reset uses a
  native revision so even a currently disabled card cannot restore stale bounds.
- Tray setup is best-effort: if Windows cannot create it, Settings reports the
  limitation and closing the main window exits instead of hiding an unreachable
  process.

### Phase 7.3 — Appearance and languages (completed)

- One versioned theme-token contract shared by trusted Atrium host
  surfaces and the optional desktop cards. Remote service WebViews remain
  intentionally isolated and keep each service's own appearance.
- Bundled `Default`, `Code`, `Translucent` and `Minimal` themes.
- System, dark and light color modes with live Windows-theme tracking.
- Persisted selection, live preview, reset and guaranteed default fallback.
- Turkish and English built-in translations.
- Locale-aware speed and capacity formatting without adding clocks or dates.

`Default` preserves the current visual language. `Code` applies a restrained
code-editor palette, optional monospace typography and syntax-like accents while
keeping the existing dashboard structure and behavior. It is not a terminal
emulator. `Translucent` is an Apple-inspired soft-surface theme, without copying
macOS. `Minimal` removes nonessential decoration and reduces visual density.

Theme Studio and safe theme packs are part of this phase:

- An Appearance page for editing safe tokens such as colors, radii, density,
  typography scale, shadows and translucency.
- Import/export through a versioned JSON format and JSON Schema.
- A repository-backed community catalog is planned after the schema becomes
  stable; automatic catalog downloads are not part of Phase 7.3.
- Theme packs may contain declarative tokens only: no arbitrary JavaScript, HTML,
  remote fonts or unrestricted CSS.

### Phase 7.4 — Simple service views (completed)

- The Phase 7.3 header, introduction and server-monitoring composition remains
  fixed; there is no full-screen dashboard editor or draggable block system.
- Existing service categories are the dashboard tabs. Changing a service's
  category in Settings moves it to that tab, and entering a new category name
  creates a new tab after saving.
- The service area can switch between the original detailed cards and a compact
  logo grid without changing service configuration or health-check behavior.
- The selected presentation is stored locally in a small versioned preference;
  invalid or unavailable browser storage falls back to the original card view.
- Service add, edit, enable/disable, icon and category controls stay in the
  existing Settings surface so the dashboard itself remains uncluttered.

### Phase 7.5 — Service discovery and icons (completed)

- Authenticated, read-only Homarr v1 application discovery through
  `GET /api/apps`, using the Phase 7.1 API-key provider. Credentials stay in the
  native vault, remain bound to the configured Homarr origin and are never
  returned to the frontend.
- Every result enters a review list. Missing URLs and existing URL/name matches
  are clearly marked and cannot be silently imported; selected entries become
  unsaved service drafts and still use the existing atomic Settings save path.
- Automatic IDs, categories, accents and icons are suggested from the service
  name, host/path and Homarr icon hint. Low-confidence matches retain a generic
  local icon and remain editable before saving.
- An expanded built-in vector catalog covers common media, download,
  automation and server-management services and works fully offline.
- Optional custom SVG/WebP icons are selected once, validated as inert image
  content, limited to 256 KiB and kept in a versioned, 32-entry/2 MiB local
  cache. Invalid storage falls back safely to the built-in icon.
- Remote Docker/Podman discovery was deliberately deferred to the restricted
  server channel and landed there as Phase 9.2.0. Exposing an unauthenticated
  Docker TCP daemon would grant much broader host control than a read-only UI
  operation implies.
- mDNS, SSDP and narrowly scoped port discovery remain optional future provider
  adapters rather than a requirement for this phase.

### Phase 7.6 — Download Center (completed)

- An optional, compact and read-only qBittorrent section: incomplete downloads,
  progress, speed, ETA, normalized state, provider badge, categories and tags.
  No configured adapter means no empty dashboard block or polling work.
- Server-side progress sorting and a 200-item cap keep completed archives from
  crowding out incomplete, paused or errored downloads. The speed summary covers
  the displayed items; one enabled provider is selected deterministically by ID.
- Username/password values stay in the native vault and are bound to the exact
  configured origin. Dynamic qBittorrent session-cookie names are retained only
  in native memory; neither credentials nor cookies reach React or repository
  files.
- Redirect-free login, bounded responses, one controlled session renewal and
  shared authentication backoff prevent hidden retry loops after an invalid
  credential or expired session.
- Sonarr/Radarr attribution is limited to exact known category/tag markers.
  Torrent names and filesystem paths are not used for heuristic guesses.
- Pause/resume, delete and all other mutations remain deferred until a separate
  explicitly authorized control phase. The provider boundary leaves room for
  later Transmission, SABnzbd and aria2 adapters.

## Phase 8 — Windows desktop integration (implemented)

- Per-user English/Turkish NSIS installer and shared application/taskbar/tray icon.
- Opt-in release-build startup registration, with uninstall cleanup.
- Single-instance activation so launching Atrium again restores the
  existing tray/background process instead of creating duplicate UI surfaces.
- Main-window size, position and maximization persistence, independent of cards.
- About shows actual native version, profile, platform and architecture. Signing
  configuration and release instructions are provided; an actual signed release
  requires the publisher's certificate and remains a distribution prerequisite.
  Signature status is explicitly unverified, with no fake update check.
- Optional server-disk, confirmed download-completion and debounced service-outage
  notifications. Native polling continues in the tray when enabled; initial
  observations are silent and generic bodies omit filenames and server addresses.
- Manual GitHub build workflow creates unsigned installer artifacts without
  publishing a release. Installed-build OS/notification smoke checks are documented.
- Desktop cards continue to represent the server, not the local Windows machine.
- No reboot or shutdown command in this phase.

## Phase 9 — Trusted server control (implementation ready; enrollment pending)

### Phase 9.0 — Restricted connection

- Optional Python-standard-library Linux Server Agent, not a dependency of the
  existing dashboard/providers. Requires a systemd server and a dedicated user.
- Single `https://<enrolled address>:9473` endpoint, manually enrolled private TLS
  public certificate, strict IP/expiry checks and a separate Windows-backed token
  vault. The address is a Settings value restricted to a bare RFC1918, loopback or
  link-local IPv4 literal; the agent port stays fixed and the stored token is bound
  to one address, so re-pointing the enrollment requires re-storing that token.
- No arbitrary endpoint, HTTP fallback, relaxed certificate checks, inherited
  proxy, redirect following, raw root password or general SSH/shell access.
- Default-disabled desktop controls and default dry-run agent. Manual deployment
  instructions, protected systemd service and two-command sudoers allowlist are
  in [server-agent/README.md](server-agent/README.md). Nothing is auto-installed.

### Phase 9.1 — Reboot/shutdown

- Main-UI-only Settings panel, explicit server-only target, two-step confirmation
  with typed IP, one-use expiring nonce and boot/mode binding.
- Server-enforced 30-second countdown and explicit cancellation before dispatch;
  graceful reboot/poweroff only, no force flag. Closing the UI does not cancel it.
- Durable local operation intent before dispatch, bounded history and read-only
  recovery. Ambiguous power requests are never automatically sent again.
- Background operation monitoring, matching journal/boot-identity reconciliation
  and verified return-online toast. Loss of reachability alone is not success.
- Narrow safety/contract tests use fake clocks, temporary journals and mocked
  dispatch. Live server enrollment, systemd/sudo compatibility and Windows toast
  delivery remain manual deployment checks; no real power action was tested.

### Phase 9.2.0 — Read-only container inventory (completed)

- Allowlisted read-only rootful Docker/Podman inventory over the existing
  restricted channel, surfaced as a second discovery source beside Homarr with
  the same review-before-add and duplicate detection.
- The raw container socket is never exposed to the desktop app or to the network
  agent. A separate administrator-installed root timer runs one fixed read-only
  list command and writes sanitized snapshots; the agent only reads those files.
- Neither the desktop nor a network request can run the exporter, choose its
  command, restart a container or change the trust boundary. Power sudoers is
  unchanged and scanning never dispatches a power command.
- Bounded contract: at most 64 containers and 8 published ports each, no image
  repository, environment, label or command line, 180-second snapshot expiry and
  explicit ready/missing/stale/unavailable/invalid source states.
- Rootful only. Rootless Podman/Docker lives in a separate user context and
  needs its own account-scoped trust boundary; it is deliberately deferred.

### Phase 9.2.1 — Optional later server management (not implemented)

- Wake-on-LAN, container/service restart and maintenance status.
- Rootless container enumeration behind an account-scoped trust boundary.

## Phase 10 — Linux desktop support

- Linux packaging and startup integration after the Windows experience and
  trusted server-control path are stable.
- Server-only companion cards adapted to the supported X11/Wayland desktop
  environments without adding local Linux hardware widgets.
- Platform-specific window-layer behavior documented and tested rather than
  relying on Windows shell or Explorer implementation details.

## Phase 11 — Mobile companion

- Reuse the validated service, monitoring, authentication and server-control
  contracts in a mobile-first Android/iOS interface.
- Start with LAN-only operation; remote access requires an explicitly configured
  trusted VPN path rather than exposing Atrium or service ports publicly.
- Android Keystore and Apple Keychain-backed secrets, with biometric confirmation
  for reboot, shutdown and other privileged actions.
- Android first; iOS builds follow when a macOS/Xcode build environment is
  available.
- Optional Android/iOS home-screen widgets only after the core mobile app is
  stable.
