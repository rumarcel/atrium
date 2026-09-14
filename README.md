# Personal Hub

A lightweight, local-first Windows desktop control center for home-server
services. Built with Tauri v2, React, TypeScript and Vite.

## Current scope

Phase 1 through Phase 8 are implemented: the desktop shell, responsive
dashboard, validated service configuration, asynchronous service health checks,
session-preserving native service tabs, live Glances monitoring and three
server-only Windows desktop cards. Phase 7.0 adds a native Settings surface,
writable per-user service configuration and Windows-backed credential storage;
Phase 7.1 adds explicit, origin-bound authentication adapters and safe native
validation state; Phase 7.2 adds the persistent opt-in card manager, tray and
safe close-to-background lifecycle; Phase 7.3 adds persisted appearance,
safe theme packs and Turkish/English localization; Phase 7.4 keeps the original
dashboard composition while adding category-based service tabs and a persistent
card/logo presentation switch. Phase 7.5 adds authenticated, read-only Homarr
application discovery, review-before-add, duplicate detection, automatic local
icon matching and bounded custom SVG/WebP storage. Service categories, icons and
visibility remain managed by the existing Settings surface rather than a full
dashboard editor.
Phase 7.6 adds a read-only qBittorrent Download Center with native session
handling, bounded retry backoff and reliable Sonarr/Radarr attribution when the
download category or tag carries an exact provider marker.
Phase 8 adds Windows startup controls, single-instance activation, main-window
geometry persistence, optional server notifications, real About/build metadata
and English/Turkish NSIS packaging. Release signing requires the publisher's own
certificate; unsigned builds are never shown as verified.
The desktop cards are separate native surfaces rather than an in-app widget editor.
The server summary now exposes Glances uptime, and an
explicit service-tab close tears down its native WebView so media cannot remain
audible invisibly. Remote Docker/Podman inventory waits for the restricted
server channel instead of requiring an exposed Docker daemon. Deeper Windows
integrations remain in later phases. The remaining
work is tracked in
[`ROADMAP.md`](ROADMAP.md).

## Architecture

- `src/components`: cross-feature React primitives and the error boundary.
- `src/features/dashboard`: dashboard composition and system preview widgets.
- `src/features/desktopWidgets`: separate server, storage and service-attention
  desktop-card surfaces plus safe window-geometry persistence.
- `src/features/backgroundRuntime`: strict runtime preference client, native
  event contract and experimental-card Settings surface.
- `src/features/appearance`: exact theme-pack parsing, bundled themes, safe
  token resolution, native persistence client and live Appearance provider.
- `src/features/health`: native health-check client, bounded polling and runtime
  status types.
- `src/features/discovery`: exact-shape Homarr discovery client, duplicate-safe
  review models and the Settings review surface.
- `src/features/downloads`: strict Download Center response parsing,
  visibility-aware polling and the compact read-only qBittorrent surface.
- `src/features/i18n`: typed English/Turkish catalogs, system-language
  resolution and locale-aware server-value formatters.
- `src/features/monitoring`: validated Glances metrics, visibility-aware polling
  and the dashboard monitoring panel.
- `src/features/services`: configuration parsing, service cards, automatic
  built-in icon matching and bounded custom icon storage.
- `src/features/settings`: service editing, recovery controls and secret-presence
  UI plus safe provider-authentication status backed by native commands.
- `src/features/tabs`: tab state, measured native viewport and the serialized
  child-WebView command client.
- `src/hooks`: shared visibility-aware polling primitives.
- `src/pages`: application-level pages.
- `src/styles`: design tokens, global rules and dashboard layout.
- `src-tauri`: Tauri v2 desktop shell, security configuration, native child
  WebView lifecycle, origin-bound credential adapters, shared desktop-card
  broker and modular Rust HTTP clients.

## Service configuration

The bundled service catalog lives at `public/config/services.json` and acts as
the validated first-run/default seed. The desktop app copies normalized settings
to its per-user Tauri application-config directory; that writable copy becomes
the source of truth on later launches. Browser-only Vite previews continue to
load the bundled file in read-only mode. The seed's JSON Schema is beside it at
`public/config/services.schema.json`.

Required service fields are `id`, `name`, `url`, `icon`, `category` and
`enabled`. `description` defaults to `Local service`, `accent` defaults to
`slate`, `tlsPolicy` defaults to `strict`, and `authentication` defaults to no
automatic adapter. Categories are user-defined and
become dashboard filters automatically. Disabled services remain valid
configuration entries but are hidden from the dashboard and health polling.

Both Rust and TypeScript reject unknown fields, invalid HTTP/HTTPS URLs, embedded URL
credentials, duplicate IDs and unsupported configuration versions. A malformed
saved file is replaced with the known-good bundled seed and produces a visible
recovery notice instead of crashing the app. Settings supports add, edit,
enable/disable and draft deletion. Saves use a same-directory temporary file and
an atomic Windows replacement; the previous valid configuration is retained as
`services.json.bak` for one-step restore. **Reset defaults** explicitly returns
to the bundled seed.

Service IDs become immutable after their first save because credential-vault
entries and isolated WebView profiles are scoped by ID. Names, descriptions,
URLs, icons, categories, accents, TLS policy, authentication policy and enabled
state remain editable. Changing a saved URL, TLS policy or browser-authentication
policy invalidates its old native child WebView and refreshes native consumers
against the new trusted catalog.

## Secure credentials

Settings can store API keys, bearer tokens, exact-origin HTTP Basic credentials
and provider username/password pairs in Windows Credential Manager. Repository
files, service URLs, settings backups and exports never contain those values.
The frontend receives only `{ kind, exists }`; stored usernames and secrets are
never returned or prefilled. Replacing a credential requires entering the full
new value, and deleting a service queues cleanup of all credential kinds owned
by that service. Before the service removal is committed, its ID is written to
an atomic, non-secret cleanup journal. Failed Credential Manager deletions are
retried at startup and around later settings changes, produce a recovery notice,
and prevent the same ID from being re-added until cleanup succeeds.

Every newly stored credential is bound to the service's normalized exact origin
(scheme, host and effective port). A Phase 7.0 credential without that binding,
or a credential whose service URL later changed, remains visible as stored but
is never transmitted; Settings asks for a replacement value instead. Provider
validation returns only closed state/reason enums and an opaque revision. It
never returns a username, secret, request header, response body or upstream
error text.

The allowlisted API adapters are Homarr `ApiKey`, Glances HTTP Basic, Glances
Bearer and qBittorrent Web API login. Exact-origin HTTP Basic is also available for WebView2's native browser
challenge flow. Credentials over plaintext HTTP are blocked unless the user
explicitly enables the local/private-network exception for that service.
Requests never follow redirects while carrying authentication, invalid attempts
use bounded backoff, and credential rotation invalidates old validation state.
Homarr validation calls its protected `/api/info` endpoint and accepts only the
bounded `{ "version": string }` response shape; Glances authentication is
applied to API-version discovery and every metrics request.

Persistent WebView profiles remain the normal browser-login mechanism. Homarr's
API key does not sign into its page, provider username/password values are not
injected into forms, and arbitrary DOM/script password entry is not allowed.
OIDC/SSO therefore continues through the service's own persistent browser
profile; no generic identity-provider credential handoff is enabled.

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
tab. Switching away from a still-open tab uses native `hide`/`show` rather than
reloading it, preserving that view's page, login state, cookies and in-memory
session. Explicitly closing a tab has different semantics: it closes and removes
the child WebView immediately. Reopening that service creates a new child view,
so a Jellyfin player or other media source cannot continue running after close.

Phase 5.1 keeps up to six still-open service WebViews warm. When capacity is
reached, the least-recently-used inactive view may be released; the active view
is never selected. Each service also uses a persistent, isolated WebView2
profile under the app's local-data directory, so ordinary persistent cookies
and local storage survive view recreation and application restarts. Session-only
cookies can still be lost after a view is released, its tab is explicitly
closed, or the full application exits. Secure integration credentials belong in
the native vault rather than in the service URL or source code.

When a service explicitly selects the browser HTTP Basic adapter, WebView2's
native authentication callback starts cancelled and supplies the bound
credential only for the configured exact origin. A credential replacement or
deletion closes that service's child view before mutating the vault so WebView2
cannot keep using cached old credentials. API-only credential changes do not
disturb the separate browser session.

The React host measures the exact workspace rectangle with `ResizeObserver` and
sends logical-pixel bounds plus a monitor-scale revision to Rust. This keeps the
native surface below the header and tab bar during window resize and per-monitor
DPI changes. Top-level service navigation is restricted to the exact configured
origin; popups, downloads, protocol changes, lookalike hosts and wrong ports are
denied. The context menu provides in-app
open, deduplicated service-tab open, validated system-browser open and URL copy.
Service editing opens Settings directly on the selected service.

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
dynamic trusted catalog and never accepts an arbitrary frontend URL.

For explicitly opted-in private HTTPS targets, Windows attaches a per-child
WebView2 certificate-error handler before navigation. It starts every callback
in a cancel state and relaxes only invalid/self-signed or local name-mismatch
errors for the configured private HTTPS origin. Expired, revoked and
client-certificate errors are cancelled whenever WebView2 reports them.
WebView2 caches an allow by host + certificate for the current session, so the
app clears those cached decisions on child creation and each tab activation;
each service/TLS policy also has its own isolated profile. This is a scoped,
explicit local-host exception—not a global certificate-ignore switch. A pinned
certificate fingerprint can tighten this further in a future dedicated trust
settings revision.

## Server monitoring

The dashboard reads Glances through the Rust backend, never directly from the
browser UI. Rust resolves the trusted `glances` entry from the saved service
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
The existing server summary shows the validated Glances uptime as a human-readable
duration; it does not synthesize an uptime value when the provider is absent.

Glances is optional. If there is no enabled `glances` catalog entry, the app
reports that server monitoring is not configured and does not continuously poll
an endpoint that cannot exist. Glances-dependent CPU, memory, storage, network,
load and uptime values remain unavailable rather than being replaced with local
Windows data or fabricated placeholders. Ordinary service-health checks remain
independent and continue to operate for every other enabled service.
Authentication secrets are not embedded in configuration. When the `glances`
service explicitly selects its HTTP Basic or Bearer adapter, every version probe
and plugin request receives the origin-bound credential in Rust. The same
redirect, transport and backoff rules apply; unauthenticated Glances remains the
default.

## Download Center

Phase 7.6 adds a compact Download Center above the service area when an enabled
service selects the `qbittorrent-web-api` adapter. If no matching provider is
configured, the section is omitted completely. The first release is deliberately
read-only: it shows incomplete downloads (including paused/error states), progress, transfer rate,
ETA, normalized state, category and tags, but exposes no pause, resume or delete
command.

The list is limited to 200 incomplete downloads, with the least-complete first.
Sorting and limiting happen on the server so a large completed/seeding archive
does not crowd out unfinished downloads. The displayed total speed is the sum
for the displayed list. When several providers are configured, the enabled
provider with the alphabetically first service ID is selected.

The frontend cannot supply a provider URL or receive the stored username,
password or session cookie. Rust selects the trusted catalog entry, performs the
form login against the exact configured origin and retains the server-selected
session-cookie name/value only in memory. Redirects are rejected, responses and
item counts are bounded, old and new qBittorrent state names are normalized and
an authentication rejection enters bounded backoff before polling can try again.
A 401/403 response invalidates the cached session and permits at most one
controlled login-and-retry for that refresh.

Sonarr/Radarr attribution is shown only for exact, case-normalized category or
tag markers such as `tv-sonarr`, `sonarr` and `radarr`; torrent names and local
filesystem paths are never guessed or returned. Custom Servarr categories remain
unattributed until a later explicit source-rule setting exists. Existing saved
catalogs are not silently rewritten: select the qBittorrent adapter and store its
provider-login credential in Settings. The bundled first-run seed already makes
that adapter available for its qBittorrent entry.
Local HTTP credential transmission remains disabled by default. For a trusted
private HTTP endpoint, explicitly enable the existing local-HTTP option in
Settings before validating the stored provider-login username and password.
This API session powers the Download Center; it does not log the embedded
qBittorrent WebUI into a browser session.

Provider behavior follows the official
[qBittorrent WebUI API](https://github.com/qbittorrent/qBittorrent/wiki/WebUI-API-%28qBittorrent-5.0%29).

## Windows desktop cards

Phase 6.1 adds three lightweight, borderless companion windows: **Server**,
**Storage** and **Service attention**. Every value describes the configured home
server; the cards do not read or display local Windows CPU, GPU, memory, battery
or storage data. They live outside the Personal Hub dashboard, stay below normal
application windows, do not appear in the taskbar and contain no clock or date.

A single Rust broker owns all polling. Server and storage cards share the same
five-second Glances sample, while the service-attention card uses the slower
health-check cycle. Each data path runs only while a card that needs it is
visible, trends are bounded to 24 in-memory samples, and each card's command
returns only its authorized data slice. Expected private self-signed TLS
exceptions remain normal online services rather than false attention items.

The experimental card system is opt-in and disabled by default. The master and
three per-card switches are persisted in a separate versioned per-user document.
When a card is not effectively enabled, its window and WebView are not created
and its broker claim does not exist. The card close button disables that card
persistently instead of hiding it until restart. Server and Storage remain
unavailable without Glances; Service attention stays independent and requires
at least one enabled health target. No card substitutes local-machine or
invented values.

Card position and size are stored per card in physical pixels. Startup validates
the saved rectangle against current monitor work areas and safely returns an
off-screen card to the primary monitor. Tray **Reset card positions** advances a
native geometry revision and recreates effective cards, so a disabled card also
ignores stale bounds when it is enabled later.

With close-to-tray enabled and at least one effective card, closing the main
window keeps those cards running. Before the main window is hidden, Rust blocks
new service-view opens and destroys every service child WebView; Jellyfin and
other media therefore cannot remain audible invisibly. A teardown failure keeps
the main window visible. The tray provides Open Personal Hub, Settings, the
master and per-card switches, geometry reset and an explicit Quit action. When
no effective card or enabled notification category exists—or close-to-tray is off—closing the main window exits
normally. Cards use Tauri's supported always-below layer; Explorer/WorkerW
desktop embedding is not enabled. If Windows tray creation fails, Settings shows
that limitation and close-to-tray is disabled so the application cannot become
an unreachable hidden process. Runtime preference writes are serialized across
processes, revision-checked and atomically replaced.

## Windows integration and packaging

Settings includes opt-in Windows startup and separate service-outage, server-disk
pressure and download-completion notifications. Windows startup registration is
available only in release builds; installation itself never enables it. Uninstall
removes Personal Hub's startup entry. Windows toast delivery requires an installed
application and depends on the user's Windows notification settings.

The native notification runtime probes only enabled categories, on a bounded
30-second schedule, including while the main window is in the tray. Initial
observations are silent. Outages require consecutive failures; disk alerts use
90%/85% trigger/rearm thresholds. Download completion must be confirmed by the
provider for a previously observed incomplete torrent, not inferred from a missing
list entry. Notifications contain generic text rather than torrent names or paths.

Launching the app again restores the existing main window, including from the
tray. Only main-window size/position/maximization is restored by the window-state
plugin; experimental-card geometry stays independent. About displays the native
version, debug/release profile, OS and architecture. Signature verification and
automatic update checks are not claimed.

Build the per-user English/Turkish NSIS installer using `pnpm build:windows`.
The bundled icon is shared by the installer, window/taskbar and tray.
See [Windows release notes](docs/windows-release.md) for the manual GitHub build
workflow, optional signing configuration and installed-build smoke checklist.
The generated local/workflow package is unsigned unless a real signing setup is
provided; certificate files are not part of the project.

## Appearance and languages

Phase 7.3 keeps the original dark `Default` appearance for first run and safe
recovery, then adds `Code`, `Translucent` and `Minimal` presets. `Code` is a
code-editor visual treatment—not a terminal emulator—and all four themes keep
the same application structure and behavior. Color mode can follow Windows or
be pinned to dark/light; system-mode changes are applied live to the main window
and the experimental desktop-card WebViews. Remote service WebView contents are
isolated and retain the service's own theme.

Appearance preferences use a separate exact, versioned per-user document with
revision-checked atomic writes. Theme Studio can preview and edit only the
allowlisted color, radius, density, type-scale, shadow, translucency and blur
tokens. Imports and exports use
[`public/config/theme-pack.schema.json`](public/config/theme-pack.schema.json);
unknown fields, unsafe CSS values, URLs, markup, scripts, remote fonts and
out-of-range values are rejected by both TypeScript and Rust. The format is a
safe future extension point for a repository-backed community catalog; this
release does not execute or automatically download third-party theme content.

English and Turkish are bundled, with an optional Windows-language mode and
English fallback for missing copy. Capacity, transfer-rate, percentage, uptime
and elapsed-duration values follow the selected locale. No clock, calendar,
date widget or visible polling timestamp is added.

## Publishing safely

Never place passwords, tokens or URL-embedded credentials in repository files.
The bundled `public/config/services.json` acts as a first-run/default seed and
can reveal local host addresses even though they are not Internet-routable. Before
publishing a reusable release, replace that seed with a sanitized example. The
user's real catalog and Credential Manager entries remain outside the repository.

## Development

```powershell
pnpm install
pnpm typecheck
pnpm test
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
