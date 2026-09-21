# ADR-008 — HTTP/JSON API, operation resources, additive `v1`

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-002, ADR-009

## Context

Clients will be a web app, a Tauri desktop app on three operating systems, and
later mobile and TV apps — some of which will be older than the Core they connect
to, and some newer. The protocol has to be writable by anyone from documentation,
debuggable with `curl`, and survivable across years of client/server skew.

Many operations are slow: pulling an image, installing a package, restarting a
container, rebooting. Some cannot be observed to completion at all. The prototype
already learned this the hard way in its power-control design, where a lost
response has to remain uncertain rather than be retried.

## Decision

### Transport

HTTP/1.1 and HTTP/2, JSON, TLS, under `/api/v1`, default port `7443`. Server-Sent
Events for the event stream. No GraphQL, no gRPC, no custom binary protocol, no
WebSocket RPC.

### Shape

- `GET` returns state and has no side effects. Changes go through resource
  create/delete or `POST .../actions/<verb>`.
- Strict parsing in both directions: unknown request fields are rejected; clients
  ignore unknown response fields.
- Closed enums for every state, kind and reason, so clients can translate them.
  Free-text status strings are never part of a contract.
- Every list is paginated or capped; every body is bounded before parsing.
- `camelCase` fields, RFC 3339 UTC times, integer seconds and bytes, no
  server-side locale formatting.

### Operations

Anything that can take longer than about a second returns `202` with an operation
id. Operations are durable resources with `queued | running | succeeded | failed |
cancelled | unknown`, progress with named steps, an actor, and a problem object on
failure.

`unknown` is a first-class outcome for work whose result cannot be observed. It is
reconciled against evidence and **never retried automatically**. Every action
accepts an `Idempotency-Key`; a repeat returns the original operation rather than
starting a second one.

### Errors

RFC 9457 `application/problem+json` with Atrium extensions: a stable machine
`code`, a user-facing `diagnosis`, a closed-enum `remediation`, `operationId`,
`requestId`, and a `details` object carrying redacted raw evidence for experts.
The same failure always produces the same `code`; codes are never reused.

### Versioning

- **Additive only within `v1`**: new fields, endpoints and enum members may appear
  at any time. Nothing is removed or repurposed. Removal requires `/api/v2` served
  in parallel.
- **Capability discovery, not version sniffing.** `GET /api/v1/system/capabilities`
  tells a client what this server can do (ADR-002). Clients never infer features
  from a version string or an OS name.
- Every response carries `X-Atrium-Api: 1` and `X-Atrium-Version: <build>`.
- An older client against a newer Core loses features, never correctness. A newer
  client against an older Core degrades using capability data; the server does not
  refuse to talk to it.

### The rule that outranks the rest

No endpoint accepts a command, argv, script, shell fragment, arbitrary unit name,
arbitrary package name, or a host path outside the platform's tree. No endpoint
returns a secret. No endpoint proxies the container runtime's API. The full list
is in [`../API.md`](../API.md#8-what-will-never-be-added-to-this-api) and it is
part of this decision, not commentary on it.

## Consequences

**Good**

- Debuggable with `curl` and readable in a browser's network panel, which matters
  enormously for a product whose users may need to diagnose it.
- Any client can be written from the documentation, including by someone who will
  never see this repository.
- Long operations do not hold connections, survive client restarts, and are
  rejoinable — closing the window cannot cancel an install.
- The error model gives `UX-STATES.md` real data to render instead of a string.
- SSE needs no protocol negotiation, survives proxies, and reconnects natively;
  polling the same resources remains a valid fallback, so a client that misses
  events still converges.

**Costs**

- More endpoints than an RPC design, and more ceremony for simple changes.
- The operation model adds a round trip and a state machine to every mutation,
  including ones that would have been instantaneous.
- "Additive only" accumulates deprecated-but-present fields over time, and `v2`
  will eventually be a real project.
- Closed enums mean adding a state is a coordinated change; clients must handle
  `unknown` members from the first release or they will break on the second.
- SSE is one-directional; anything needing client-to-server streaming (there is
  nothing today) would need a different mechanism.

## Alternatives considered

**gRPC.** Better typing, worse everything else here: browser support needs a
proxy, it is not debuggable by hand, and it raises the cost of a third-party
client. Rejected.

**GraphQL.** Attractive for a dashboard fetching many small things. Rejected:
query-cost control becomes a security problem on a 2 GB machine, the mutation
model fights the operation model, and it is far heavier than this surface needs.

**WebSocket RPC for everything.** Rejected: reimplements HTTP semantics badly,
complicates authorization and auditing, and makes the API undebuggable with
ordinary tools.

**Synchronous long requests.** Rejected: breaks on any proxy timeout, loses state
on client restart, and makes `unknown` outcomes indistinguishable from failures.

**Unversioned API with rolling compatibility.** Rejected: skew is guaranteed once
native clients exist and upgrade on their own schedule.

**Semantic version negotiation instead of capability discovery.** Rejected: a
version cannot express "this machine has no SMART-capable disk", which is the
question clients actually need answered.
