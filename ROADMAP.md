# Personal Hub roadmap

This roadmap records product direction rather than promising fixed release dates.
The current dark Personal Hub appearance remains the initial and fallback theme.

## Product decisions

- Do not add a dashboard clock, calendar or visible polling timestamps. Internal
  timestamps remain available only for scheduling, freshness and stale-data logic.
- Theme changes apply to Personal Hub's dashboard, navigation and settings. Remote
  service pages run in isolated native WebViews and keep their own appearance.
- Invalid or missing appearance settings always fall back to the bundled default.

## Completed foundation

- Phases 1-4: desktop shell, dashboard, validated service catalog and health checks.
- Phase 5: native service tabs.
- Phase 5.1: warm WebView pool and persistent per-service profiles.
- Phase 5.2: service-player fullscreen integration.
- Phase 6: Glances server monitoring.
- Phase 6.1: server-only Windows desktop cards.

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
- Hide-until-restart behavior; persistent visibility and reset controls move to
  the Phase 8 tray manager.
- No local Windows hardware telemetry, clock/date or credential-dependent
  provider integration.

## Phase 7 — Settings and integrations

### Phase 7.0 — Settings and credential vault

- Writable per-user service configuration with validated migration and recovery.
- Service add, edit, remove, enable and disable flows.
- Persistent application preferences with an explicit reset path.
- Windows-backed secret storage for services that require authentication.
- Credentials remain outside repository files, URLs, logs and exported settings.

### Phase 7.1 — Appearance and languages

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

### Phase 7.2 — Service discovery and icons

- Homarr import and read-only Docker/Podman inventory.
- Later mDNS, SSDP and optional narrowly scoped port discovery.
- Review-before-add flow, duplicate detection and confidence levels.
- A local Dashboard Icons-based SVG/WebP catalog with dark/light variants.
- Custom icon import with validated storage and a bounded cache.

### Phase 7.3 — Download Center

- A read-only qBittorrent provider first: active downloads, progress, speed, ETA,
  state, provider badge, categories and tags.
- Sonarr/Radarr source attribution where it can be determined reliably.
- Controlled session renewal and backoff after invalid credentials.
- Pause/resume only after the read-only path is proven safe.
- Adapter path for Transmission, SABnzbd and aria2.

## Phase 8 — Windows desktop integration

- NSIS installer, application/taskbar icons, tray and startup registration.
- Configurable close-to-tray behavior and window geometry persistence.
- Tray controls to show, hide and reset the server desktop cards.
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

## Phase 10 — Linux desktop support (final platform phase)

- Linux packaging and startup integration after the Windows experience and
  trusted server-control path are stable.
- Server-only companion cards adapted to the supported X11/Wayland desktop
  environments without adding local Linux hardware widgets.
- Platform-specific window-layer behavior documented and tested rather than
  relying on Windows shell or Explorer implementation details.
