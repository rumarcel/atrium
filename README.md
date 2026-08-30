# Personal Hub

A lightweight, local-first Windows desktop control center for home-server
services. Built with Tauri v2, React, TypeScript and Vite.

## Current scope

Phase 1 through Phase 6 are implemented: the desktop shell, responsive
dashboard, validated service configuration, asynchronous service health checks,
session-preserving native service tabs and live Glances monitoring. The dashboard
keeps its original Phase 6 composition; Phase 6.1 desktop cards will be separate
Windows surfaces rather than an in-app widget editor. Settings and deeper Windows
integrations remain intentionally out of scope for this phase.
Planned settings, themes and later integrations are tracked in
[`ROADMAP.md`](ROADMAP.md).

## Architecture

- `src/components`: cross-feature React primitives and the error boundary.
- `src/features/dashboard`: dashboard composition and system preview widgets.
- `src/features/health`: native health-check client, bounded polling and runtime
  status types.
- `src/features/monitoring`: validated Glances metrics, visibility-aware polling
  and the dashboard monitoring panel.
- `src/features/services`: configuration parsing, loading state and service cards.
- `src/features/tabs`: tab state, measured native viewport and the serialized
  child-WebView command client.
- `src/hooks`: shared visibility-aware polling primitives.
- `src/pages`: application-level pages.
- `src/styles`: design tokens, global rules and dashboard layout.
- `src-tauri`: Tauri v2 desktop shell, security configuration, native child
  WebView lifecycle and modular Rust HTTP clients.

## Service configuration

The bundled service catalog lives at `public/config/services.json`. Vite copies
it to `dist/config/services.json`, and the dashboard loads it at runtime instead
of embedding host addresses in TypeScript. Its JSON Schema is beside it at
`public/config/services.schema.json`.

Required service fields are `id`, `name`, `url`, `icon`, `category` and
`enabled`. `description` defaults to `Local service`, `accent` defaults to
`slate`, and `tlsPolicy` defaults to `strict`. Categories are user-defined and
become dashboard filters automatically. Disabled services remain valid
configuration entries but are hidden from the dashboard and health polling.

The loader rejects unknown fields, invalid HTTP/HTTPS URLs, embedded URL
credentials, duplicate IDs and unsupported configuration versions. A malformed
file produces a recoverable error panel instead of crashing the app.

This file is currently an application-bundled seed. Editing it changes the next
development or production build. Phase 7 will seed a writable per-user copy in
the Tauri application config directory for Settings persistence.

## Service health checks

Health checks run through an asynchronous Rust command, not browser `fetch`, so
they are not affected by service CORS policies and do not block the React UI.
Enabled services are checked at startup, after each completed cycle every 45
seconds, and on demand with **Refresh status**. At most eight checks run at
once. Each request has a 2.5-second timeout, does not follow redirects, and
reads only the response head. HTTP 200–399 is considered reachable.

TLS certificate validation is strict by default. The optional
`allow-invalid-local-certificate` policy is accepted only for an HTTPS
loopback, RFC1918/private, link-local or IPv6 unique-local target. It creates a
separate, narrowly selected client and is never used for public addresses or
ordinary hostnames. A reachable service checked with this exception is shown as
a yellow `Local TLS` warning rather than a fully verified green result. The four
bundled self-signed services opt in explicitly.

Browser-only Vite previews show health checks as desktop-only; native status is
available when the frontend runs inside Tauri.

## Native service tabs

Selecting a service creates a real WebView2 child surface inside the Tauri
window. Dashboard is a permanent first tab and each service has at most one open
tab. Switching tabs and closing a tab use native `hide`/`show` rather than
reloading or immediately destroying the child. This preserves the page, login
state, cookies and in-memory session when the service is reopened.

Phase 5.1 keeps up to six service WebViews warm. When capacity is reached, the
least-recently-used inactive view is released, preferring already closed tabs;
the active view is never selected. Each service also uses a persistent,
isolated WebView2 profile under the app's local-data directory, so ordinary
persistent cookies and local storage survive view recreation and application
restarts. Session-only cookies can still be lost after an LRU release or full
application exit. Securely stored HTTP/basic-auth credentials belong in Phase 7
rather than in the service URL or source code.

The React host measures the exact workspace rectangle with `ResizeObserver` and
sends logical-pixel bounds plus a monitor-scale revision to Rust. This keeps the
native surface below the header and tab bar during window resize and per-monitor
DPI changes. Top-level service navigation is restricted to the exact configured
origin; popups, downloads, protocol changes, lookalike hosts and wrong ports are
denied. The context menu provides in-app
open, deduplicated new-tab open, validated system-browser open and URL copy.
Service editing remains disabled until Phase 7.

Phase 5.2 follows WebView2's native HTML fullscreen transition through the
parent Tauri window. While an active service is fullscreen, the application
header and tab bar are removed from layout and the existing bounds observer
expands the child WebView over the full client area. Leaving fullscreen restores
the chrome and remeasures the child without reloading it. Browser-only previews
remain on the regular dashboard layout.

Only the trusted `main` UI WebView receives Tauri capabilities. Remote
`service-*` WebViews receive no IPC capability or remote URL grant, and each
privileged command also verifies the caller label. Native open/browser commands
accept only a service id; Rust resolves the name, URL and TLS policy from the
bundled trusted catalog and never accepts an arbitrary frontend URL.

For explicitly opted-in private HTTPS targets, Windows attaches a per-child
WebView2 certificate-error handler before navigation. It starts every callback
in a cancel state and relaxes only invalid/self-signed or local name-mismatch
errors for the configured private HTTPS origin. Expired, revoked and
client-certificate errors are cancelled whenever WebView2 reports them.
WebView2 caches an allow by host + certificate for the current session, so the
app clears those cached decisions on child creation and each tab activation;
each service/TLS policy also has its own isolated profile. This is a scoped,
explicit local-host exception—not a global certificate-ignore switch. A pinned
certificate fingerprint can tighten this further when editable trust settings
are introduced in Phase 7.

## Server monitoring

The dashboard reads Glances through the Rust backend, never directly from the
browser UI. Rust resolves the trusted `glances` entry from the bundled service
catalog, applies the same explicit local TLS policy, rejects redirects, limits
each response to 1 MiB and uses a 2.5-second request timeout. The frontend cannot
supply or override the monitoring URL.

Glances REST API v4 is detected first, with a v3 fallback only when the v4 API
is not present. CPU, memory, sensors, network, filesystem, uptime and load
plugins are fetched concurrently as seven small requests. Network units are
normalized across API versions, loopback and common virtual adapters are
excluded when a physical adapter is available, CPU-oriented temperature sensors
are preferred and
pseudo/bind-mounted filesystems are filtered and deduplicated.

Metrics update every five seconds only while the dashboard is active and the
document is visible. Requests never overlap. Service
health checks use the same polling lifecycle at their slower interval. A failed
refresh keeps the last successful sample marked as stale; an initial failure
produces an `Unavailable` panel instead of affecting the rest of the dashboard.
Authentication secrets are not embedded in configuration. If Glances requires
credentials, the monitoring panel reports that secure credential support is planned for
Phase 7.

## Desktop-card direction

Phase 6.1 is reserved for lightweight cards that live in their own Windows
desktop surfaces. It does not replace, reorder or resize sections inside the
Personal Hub dashboard. The monitoring backend already normalizes optional
uptime and one-, five- and fifteen-minute load averages and keeps a bounded
in-memory trend window as groundwork for those desktop cards. No sample date or
clock is rendered.

## Publishing safely

Never place passwords, tokens or URL-embedded credentials in repository files.
The bundled `public/config/services.json` currently acts as a build-time seed and
can reveal local host addresses even though they are not Internet-routable. Before
publishing a reusable release, replace that seed with a sanitized example. Phase 7
will move the user's real catalog into the per-user application-data directory.

## Development

```powershell
pnpm install
pnpm typecheck
pnpm build
cd src-tauri
cargo test
cd ..
pnpm tauri dev
```

To verify the native application without producing an installer:

```powershell
pnpm tauri build --no-bundle
```
