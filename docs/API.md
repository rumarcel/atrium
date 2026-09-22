# Atrium API

Status: target design of the `v1` surface. Paths and payloads below are a
starting contract, not a frozen specification — the frozen parts are the
principles in §1, the error model in §4 and the operation model in §5.

## 1. Principles

1. **Narrow domain actions, never generic execution.** Every endpoint names a
   thing the product does. There is no endpoint that takes a command, a script, a
   unit name, a package name outside an allowlist, or a path outside the
   platform's own tree. This is the hardest rule in the project.
2. **Resources for state, actions for verbs.** `GET` returns state. Changes go
   through `POST .../actions/<verb>` or a resource create/delete, never through a
   `GET` with side effects.
3. **Anything slow is an operation.** If it can take longer than about a second —
   an install, a pull, a restart, a backup, a power action — it returns `202` with
   an operation id and progresses observably. Requests are never held open.
4. **Capability discovery, not version sniffing.** A client asks what this server
   can do; it does not infer it from a version string or an OS name.
5. **Additive within a major version.** New fields and new enum members may appear
   at any time. Clients ignore unknown fields and map unknown enum members to a
   defined `unknown` case.
6. **Typed, closed enums.** Every state, reason and kind is a fixed vocabulary the
   UI can translate. Free-text status strings are not part of any contract — the
   prototype's closed `state`/`reason` enums are the model.
7. **Nothing sensitive is ever returned.** Secrets, tokens, passwords, cookies and
   upstream error bodies do not appear in any response. Credential state is
   `{ kind, exists }` and nothing more.
8. **Bounded everything.** Every list is paginated or capped; every response has a
   maximum size; every request body has a maximum size checked before parsing.

## 2. Transport and conventions

| Property | Value |
| --- | --- |
| Base path | `/api/v1` |
| Transport | HTTPS. TLS 1.2 is the floor for the API in general; **TLS 1.3 is required for the three pairing endpoints** as a deliberate baseline (ADR-003 §4). No plaintext listener except an optional loopback-only debug port that is off by default. |
| Default port | `7443` |
| Content type | `application/json; charset=utf-8`; `text/event-stream` for events; `application/octet-stream` for downloads |
| Auth | `Authorization: Bearer <device token>`, or session cookie + `X-Atrium-CSRF` for browsers |
| Idempotency | `Idempotency-Key` header on every action; a repeat returns the original operation |
| Correlation | `X-Request-Id` accepted and echoed; generated when absent; present in audit records |
| Version headers | `X-Atrium-Api: 1`, `X-Atrium-Version: <build>` on every response |
| Time | RFC 3339 UTC strings. Durations in seconds as integers. Sizes in bytes as integers. No locale formatting server-side. |
| Casing | `camelCase` field names, matching the prototype's existing serde conventions |
| Unknown fields in requests | Rejected. Strict parsing, both directions. |

Pagination: `?limit=` (default 50, max 200) and `?cursor=`; responses carry
`{ items, nextCursor }`. Never an unbounded array.

## 3. Authentication and pairing surface

The only endpoints reachable without authentication. All are rate limited and all
are safe to expose on the LAN.

```http
GET  /api/v1/pair/info
POST /api/v1/pair/begin
POST /api/v1/pair/complete
GET  /healthz
```

```jsonc
// GET /api/v1/pair/info
{
  "serverId": "8f14e45fceea167a5a36dedd4bea2543",
  "name": "atrium",
  "version": "0.1.0",
  "spki": "sha256:9b74c9897bac770ffc029102a200c5de",   // for display and pin confirmation
  "claimed": false,
  "pairingOpen": true
}
```

```jsonc
// POST /api/v1/pair/begin
//   { "deviceName": "Study laptop", "platform": "windows",
//     "clientNonce": "<32B base64>",
//     "bindingProfile": "atrium-pair-binding/native-tls-exporter-v1" }
{ "pairingId": "c9f0f895fb98ab9159f51fd0297e236d", "serverNonce": "<32B base64>", "expiresAt": "2026-09-22T18:40:00Z" }

// POST /api/v1/pair/complete { "pairingId": "...", "proofC": "<base64 HMAC-SHA256>" }
{ "proofS": "<base64 HMAC-SHA256>",                  // the client verifies this BEFORE storing anything
  "deviceId": "...", "deviceToken": "<opaque 32B base64>", "role": "owner",
  "server": { "id": "...", "name": "..." } }
```

Both proofs are computed over a transcript that names its **binding profile** and,
for the native profile, includes the server's SPKI hash and an RFC 8446 TLS
exporter, so the exchange is bound to this specific TLS connection and this
specific server key. The profile is **not negotiated**: an armed secret permits
exactly the profiles the operator armed it for, and a request naming any other is
refused with the same generic `pairing_rejected`. Alpha arms and implements only
`native-tls-exporter-v1`; the browser profile exists in the transcript so the web
client is an addition rather than a replacement, and is gated on the certificate
decision in A-24 (ADR-003 §4a). The construction, the mandatory secret
format (16 random bytes, 128 bits, 26 Crockford Base32 symbols) and every
lifecycle rule are specified in
[ADR-003](adr/0003-authentication-and-device-pairing.md); implementing this
endpoint without reading it is a security bug.

Failure returns a single generic `pairing_rejected` code regardless of which part
was wrong, with a `retryAfter` when rate limited.

Authenticated identity:

```http
GET    /api/v1/me                      # this device, its role, the user it belongs to
GET    /api/v1/devices
DELETE /api/v1/devices/{deviceId}      # revoke; immediate
POST   /api/v1/devices/actions/revoke-all
```

## 4. Error model

Every non-2xx response is `application/problem+json` (RFC 9457) with Atrium
extensions:

```jsonc
{
  "type": "https://atrium.local/errors/app-port-conflict",
  "title": "The port Jellyfin needs is already in use",
  "status": 409,
  "code": "app.port_conflict",              // stable machine identifier, never localised
  "diagnosis": "Port 8096 is already bound by another container on this server.",
  "remediation": "choose_different_port",   // closed enum the UI maps to an action
  "operationId": "b2f4...",                 // when the failure belongs to an operation
  "requestId": "3f1c...",
  "details": {                              // expert view; present when there is raw evidence
    "provider": "container.docker",
    "conflictingContainer": "jellyfin-old",
    "port": 8096
  }
}
```

Rules:

- `code` is stable and machine-readable. UI copy is chosen by the client from
  `code`, not by displaying `title` blindly — `title` is the fallback.
- `diagnosis` explains the cause in the user's terms; `remediation` names what can
  be done about it.
- `details` is the expert panel's content: exit statuses, provider names, bounded
  stderr excerpts. It is redacted by allowlist and never contains secrets.
- The same failure always produces the same `code`. A new failure mode gets a new
  code; codes are never reused or repurposed.

Reserved code families: `auth.*`, `pairing.*`, `capability.*`, `provider.*`,
`app.*`, `container.*`, `agent.*`, `storage.*`, `network.*`, `operation.*`,
`validation.*`, `internal.*`.

Three codes exist specifically for the container boundary and are never merged
into a generic failure:

| Code | Meaning |
| --- | --- |
| `container.not_managed` | The target is not in Agent's ownership registry. Atrium did not create it and will not touch it. |
| `container.ownership_mismatch` | A registry entry exists but the live container's owner token or labels do not match. The app becomes `disowned` and waits for the owner. |
| `agent.spec_refused` | Agent refused the derived specification — a permission the Alpha constraint set does not allow. |
| `agent.catalogue_signature_invalid` | The catalogue index signature did not verify against Agent's compiled-in publisher trust root. No API override exists. |
| `agent.manifest_digest_mismatch` | The manifest does not match the digest recorded in the verified catalogue index. |

## 5. Operations and events

```http
GET  /api/v1/operations?state=active
GET  /api/v1/operations/{operationId}
POST /api/v1/operations/{operationId}/actions/cancel
GET  /api/v1/events                      # Server-Sent Events
```

```jsonc
// GET /api/v1/operations/{id}
{
  "id": "b2f4...",
  "kind": "app.install",
  "target": { "type": "app", "id": "jellyfin" },
  "state": "running",                    // queued | running | succeeded | failed | cancelled | unknown
  "progress": { "step": "pull", "stepIndex": 3, "stepCount": 6, "percent": 41 },
  "startedAt": "2026-09-22T18:31:04Z",
  "finishedAt": null,
  "cancellable": true,
  "error": null,                          // a problem object when state = failed
  "actor": { "deviceId": "...", "role": "owner" }
}
```

- `unknown` is a real terminal-ish state, used when Atrium cannot observe the
  outcome (a reboot whose response was lost). It is reconciled against evidence —
  boot identity, container state — and **never** retried automatically. This is
  the prototype's power-control rule promoted to a platform rule.
- Cancellation is best-effort and honest: an operation that has passed its point
  of no return reports `cancellable: false` rather than pretending.

The SSE stream carries `operation.updated`, `app.state.changed`,
`system.metrics`, `capability.changed`, `notification.raised` and `heartbeat`.
Polling the same resources is always a valid fallback; the event stream is an
optimisation, never a requirement, because a client that misses events must still
converge.

## 6. Resource surface

### System

```http
GET /api/v1/system                 # identity, os, kernel, arch, hostname, uptime, boot id, time
GET /api/v1/system/capabilities    # what this host can actually do — see below
GET /api/v1/system/metrics         # current CPU, memory, load, temperature, network throughput
GET /api/v1/system/metrics/history?window=1h&resolution=1m
GET /api/v1/system/diagnostics     # provider selection, versions, last errors
POST /api/v1/system/diagnostics/actions/bundle   # 202 -> downloadable redacted bundle
POST /api/v1/system/actions/reboot               # destructive: requires confirmation token
POST /api/v1/system/actions/shutdown             # destructive: requires confirmation token
```

```jsonc
// GET /api/v1/system/capabilities — the contract that drives every "unavailable" state in the UI
// Agent-side capabilities are reported by Agent and relayed; Core does not probe them itself.
{
  "container": { "available": true,  "provider": "docker", "version": "27.3.1", "via": "agent",
                 "features": ["managed-apps", "inventory"],
                 "missing": [{ "feature": "device-passthrough", "reason": "not_permitted_in_alpha" }] },
  "packages":  { "available": false, "missing": [{ "feature": "install", "reason": "not_enabled_in_alpha" }] },
  "services":  { "available": true,  "provider": "systemd", "version": "252", "via": "agent" },
  "storage":   { "available": true,  "provider": "linux", "features": ["filesystems"],
                 "missing": [{ "feature": "smart", "reason": "no_smart_capable_device" }] },
  "network":   { "available": true,  "provider": "linux-netlink" },
  "hardware":  { "available": true,  "features": ["cpu", "memory"],
                 "missing": [{ "feature": "temperature", "reason": "no_hwmon_sensors" }] },
  "privileged":{ "available": true,  "agentVersion": "0.1.0" }
}
```

### Storage

```http
GET /api/v1/storage                       # summary: total, used, per-mount rollup
GET /api/v1/storage/filesystems
GET /api/v1/storage/disks                 # 0.2
GET /api/v1/storage/disks/{id}/smart      # 0.2
GET /api/v1/storage/shares                # 0.2
```

### Network

```http
GET /api/v1/network/interfaces            # every NIC, every address — never "the" IP
GET /api/v1/network/addresses             # what clients can reach this server on
```

### Applications

```http
GET    /api/v1/apps
GET    /api/v1/apps/{appId}
POST   /api/v1/apps                       # install from a catalogue entry -> 202 operation
DELETE /api/v1/apps/{appId}?data=keep|remove   # -> 202 operation, destructive confirmation for remove
POST   /api/v1/apps/{appId}/actions/start
POST   /api/v1/apps/{appId}/actions/stop
POST   /api/v1/apps/{appId}/actions/restart
POST   /api/v1/apps/{appId}/actions/upgrade
GET    /api/v1/apps/{appId}/logs?tail=200&since=...   # bounded; streamable via SSE
GET    /api/v1/apps/{appId}/settings
PUT    /api/v1/apps/{appId}/settings      # validated against the manifest schema
GET    /api/v1/catalog/apps
GET    /api/v1/catalog/apps/{catalogId}
```

```jsonc
// GET /api/v1/apps/jellyfin
{
  "id": "jellyfin",
  "catalogId": "jellyfin",
  "name": "Jellyfin",
  "manifestRevision": 4,
  "version": "10.10.3",
  "state": "running",            // installing | running | stopped | unhealthy | upgrading | failed | uninstalling | disowned | unknown
  "health": { "state": "healthy", "since": "2026-09-22T18:33:10Z" },
  "entry": { "url": "https://192.0.2.10:8096/", "openIn": "embedded" },
  "ports": [{ "id": "http", "host": 8096, "container": 8096, "protocol": "tcp" }],
  "volumes": [{ "id": "config", "kind": "appdata", "usedBytes": 41231872 }],
  "updateAvailable": { "version": "10.10.4", "manifestRevision": 5, "permissionsChanged": false },
  "managed": true
}
```

### Containers (unmanaged)

Containers Atrium did not create. **Read-only, with no exceptions** — no start, no
stop, no restart, no logs, no removal. They belong to the user (A-16), and the
restriction is structural rather than a policy check: mutation operations in the
Core-to-Agent protocol take an `appId`, and an unmanaged container does not have
one (ADR-013).

```http
GET /api/v1/containers          # sanitized inventory, managed and unmanaged
GET /api/v1/containers/{id}     # sanitized detail for display
```

The inventory returns id, name, image reference, state, published ports and a
`managed` flag. It does not return environment variables, labels beyond the
managed marker, command lines, or mount sources — the same bounded, sanitized
shape the prototype's inventory exporter already produces.

Adoption — the owner deliberately handing an existing container to Atrium — is out
of Alpha scope and will not be an API-only action when it arrives (ADR-014).

### External links

The prototype's service catalog, relocated to Core so every client sees the same
list. Purely descriptive: a name, a URL, an icon, a category, a health check.

```http
GET    /api/v1/links
POST   /api/v1/links
PUT    /api/v1/links/{id}
DELETE /api/v1/links/{id}
GET    /api/v1/links/{id}/health
POST   /api/v1/links/import              # from Homarr, or from container inventory
```

### Users, audit, settings, updates

```http
GET    /api/v1/users                     # post-Alpha
GET    /api/v1/audit?since=&actor=&action=
GET    /api/v1/settings
PUT    /api/v1/settings
GET    /api/v1/updates                   # platform update availability
POST   /api/v1/updates/actions/apply     # -> 202 operation, staged with rollback
GET    /api/v1/notifications
```

## 7. Destructive confirmation

Any operation that can destroy data or interrupt service requires a two-step
exchange, generalising the prototype's typed-address confirmation:

```http
POST /api/v1/confirmations          { "action": "app.uninstall", "target": "jellyfin", "options": { "data": "remove" } }
  -> 200 { "confirmationId": "...", "expiresAt": "...", "consequences": ["app.data_deleted", "app.stops"] }

DELETE /api/v1/apps/jellyfin?data=remove
  X-Atrium-Confirmation: <confirmationId>
```

The confirmation is single-use, short-lived, bound to the exact action, target and
options, and consumed server-side. A mismatch between the confirmed options and
the executed request is a hard failure. `consequences` is a closed enum the client
renders as the warning text, so the warning cannot drift from what actually
happens.

## 8. What will never be added to this API

For the avoidance of future debate:

- `POST /exec`, `POST /shell`, `POST /run`, or any endpoint accepting a command.
- Proxying the Docker socket, the Docker HTTP API, or any runtime's raw API — in
  either direction, and including a "filtered" or "read-only" proxy. Core cannot
  reach the runtime at all (ADR-013), so there is nothing to proxy.
- Any endpoint that takes a container identifier and mutates it. Mutation names an
  app; Agent resolves it through its own ownership registry.
- Any endpoint that starts, stops, restarts or removes a container Atrium did not
  create, under any role, including `owner`.
- An endpoint returning a stored secret, password, token or session cookie.
- An endpoint that accepts an arbitrary systemd unit name or package name.
- An endpoint that takes a host filesystem path from the client and acts on it
  outside the platform's own tree.
- A client-supplied URL that Core will fetch on the client's behalf, except where
  it is validated against an allowlisted, owner-configured integration — the
  prototype's rule that the frontend can never supply a monitoring or provider
  URL.
