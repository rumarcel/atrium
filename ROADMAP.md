# Personal Hub roadmap

This roadmap records product direction rather than promising fixed release dates.
The current dark Personal Hub appearance remains the initial and fallback theme.

## Product decisions

- Do not add a dashboard clock, calendar or visible polling timestamps. Internal
  timestamps remain available only for scheduling, freshness and stale-data logic.
- Theme changes apply to Personal Hub's dashboard, navigation and settings. Remote
  service pages run in isolated native WebViews and keep their own appearance.
- Invalid or missing appearance settings always fall back to the bundled default.
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

## Phase 6.1 — Windows desktop cards (completed)

- Three independent companion surfaces for server metrics, server storage and
  server-service attention, outside the Personal Hub dashboard.
- The in-app dashboard remains in its Phase 6 layout with no widget editor,
  reordering or card-size controls.
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
  `Open Personal Hub` and `Quit` actions.
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

### Phase 7.3 — Appearance and languages

- One versioned theme-token contract shared by every Personal Hub component.
- Bundled `Default`, `Terminal`, `Translucent` and `Minimal` themes.
- System, dark and light color modes with live Windows-theme tracking.
- Persisted selection, live preview, reset and guaranteed default fallback.
- Turkish and English built-in translations.
- Locale-aware speed and capacity formatting without adding clocks or dates.

`Default` preserves the current visual language. `Terminal` uses a restrained
code-editor aesthetic. `Translucent` is an Apple-inspired soft-surface theme,
without copying macOS. `Minimal` removes nonessential decoration and reduces
visual density.

Theme Studio and safe theme packs are part of this phase:

- An Appearance page for editing safe tokens such as colors, radii, density,
  typography scale, shadows and translucency.
- Import/export through a versioned JSON format and JSON Schema.
- A repository-backed community catalog after the schema becomes stable.
- Theme packs may contain declarative tokens only: no arbitrary JavaScript, HTML,
  remote fonts or unrestricted CSS.

### Phase 7.4 — Service discovery and icons

- Authenticated Homarr import using the Phase 7.1 provider plus read-only
  Docker/Podman inventory.
- Later mDNS, SSDP and optional narrowly scoped port discovery.
- Review-before-add flow, duplicate detection and confidence levels.
- A local Dashboard Icons-based SVG/WebP catalog with dark/light variants.
- Custom icon import with validated storage and a bounded cache.

### Phase 7.5 — Download Center

- A read-only qBittorrent provider first: active downloads, progress, speed, ETA,
  state, provider badge, categories and tags.
- Sonarr/Radarr source attribution where it can be determined reliably.
- Controlled session renewal and backoff after invalid credentials.
- Pause/resume only after the read-only path is proven safe.
- Adapter path for Transmission, SABnzbd and aria2.

## Phase 8 — Windows desktop integration

- NSIS installer, application/taskbar icons and startup registration.
- Single-instance activation so launching Personal Hub again restores the
  existing tray/background process instead of creating duplicate UI surfaces.
- Signed-release and installed-version visibility so the running build can be
  identified from an About/update page.
- Disk, download and service-outage notifications.
- Desktop cards continue to represent the server, not the local Windows machine.
- No reboot or shutdown command in this phase.

## Phase 9 — Trusted server control

- A narrowly scoped Personal Hub Server Agent or restricted SSH account.
- Reboot and shutdown target only the configured `192.168.1.10` server.
- Main-UI-only commands, explicit target display, two-step confirmation and countdown.
- No raw root password or unrestricted SSH access.
- Local operation history, reboot/offline tracking and return-online notification.
- Optional later Wake-on-LAN, container/service restart and maintenance status.

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
  trusted VPN path rather than exposing Personal Hub or service ports publicly.
- Android Keystore and Apple Keychain-backed secrets, with biometric confirmation
  for reboot, shutdown and other privileged actions.
- Android first; iOS builds follow when a macOS/Xcode build environment is
  available.
- Optional Android/iOS home-screen widgets only after the core mobile app is
  stable.
