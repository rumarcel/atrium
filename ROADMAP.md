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

## Phase 7 — Settings and personalization

### Phase 7.0 — Settings foundation

- Writable per-user service configuration with validated migration and recovery.
- Service add, edit, remove, enable and disable flows.
- Persistent application preferences with an explicit reset path.

### Phase 7.1 — Secure credentials

- Windows-backed secret storage for services that require authentication.
- Credentials remain outside repository files, URLs, logs and exported settings.

### Phase 7.2 — Appearance engine

- One versioned theme-token contract shared by every Personal Hub component.
- Bundled `Default`, `Terminal`, `Translucent` and `Minimal` themes.
- Dark/light palette variants where the theme supports both.
- Persisted selection, live preview, reset and guaranteed default fallback.

`Default` preserves the current visual language. `Terminal` uses a restrained
code-editor aesthetic. `Translucent` is an Apple-inspired soft-surface theme,
without copying macOS. `Minimal` removes nonessential decoration and reduces
visual density.

### Phase 7.3 — Theme Studio and theme packs

- An Appearance page for editing safe tokens such as colors, radii, density,
  typography scale, shadows and translucency.
- Import/export through a versioned JSON format and JSON Schema.
- A repository-backed community catalog after the schema becomes stable.
- Theme packs may contain declarative tokens only: no arbitrary JavaScript, HTML,
  remote fonts or unrestricted CSS.

## Later phases

- Phase 8: Windows behavior, installer, tray, startup and window persistence.
- Phase 9: trusted local service discovery and service-logo metadata.
- Phase 10: extensible server/download widgets and source attribution.
- Phase 11: localization and translated built-in content.
