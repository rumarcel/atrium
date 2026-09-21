# ADR-013 — The container runtime belongs to the Agent

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-001, ADR-002, ADR-004, ADR-005, ADR-014
**Assumptions:** A-16

## Context

The first version of this architecture put `ContainerProvider` inside Core and had
Core join the `docker` group. `SECURITY.md` then had to admit, in its residual-risk
section, that this made a Core compromise equivalent to a host compromise.

That admission is not a caveat. It contradicts the primary property the whole
Core/Agent split exists to provide. Membership of the `docker` group — or any
access to `/var/run/docker.sock`, a Docker TCP endpoint, or a permissive socket
proxy — is root on the host: anyone who can talk to the daemon can start a
privileged container that mounts `/` and writes to it.

Keeping that access in Core meant the architecture had two contradictory claims in
it: "Core is unprivileged" and "Core can talk to the container daemon". Only one
of them can be true.

There is a second, independent problem. Atrium runs on machines that already have
containers the user created. Those belong to the user. If Core decides which
containers are Atrium's, then a compromised Core simply declares that the user's
database container is Atrium's and removes it. An ownership rule that only Core
enforces is not an ownership rule.

## Decision

### 1. Core has no container-runtime access of any kind

Core is **not** a member of the `docker` group, does not open
`/var/run/docker.sock`, does not connect to a Docker TCP socket, is not given a
socket proxy, and does not obtain equivalent unrestricted runtime privilege by any
other route. Core's unit sets `SupplementaryGroups=` empty, so this is enforced by
configuration and not by convention.

### 2. Agent owns the runtime connection

`ContainerProvider` executes inside Agent. Agent is the only process on the machine
holding Atrium's runtime socket access. The call chain is:

```
UI → Core → typed Agent operation → ContainerProvider → Docker / future Podman
```

### 3. Core expresses domain intent, not runtime calls

The Core-to-Agent protocol has container operations shaped like product features,
not like a runtime API (ADR-004):

`ContainerInventory`, `RuntimeInfo`, `AppApply`, `AppStart`, `AppStop`,
`AppRestart`, `AppRemove`, `AppInspect`, `AppLogs`.

**This must never become a Docker API passthrough.** Concretely, the protocol
contains no operation that names a runtime endpoint, no operation that forwards an
opaque request body, no `exec`, no `attach`, no raw `create` taking a
runtime-native specification, and no container identifier as an input parameter.
Adding one is a superseding ADR, not an implementation decision.

### 4. Agent enforces ownership with its own evidence

Core saying "this container is Atrium-managed" is not evidence. Agent keeps its
own, in `/var/lib/atrium-agent/ownership.json` (`0600 root:root`, atomic replace,
unreadable and unwritable by the `atrium` user):

```jsonc
{ "appId": "jellyfin",
  "containerId": "sha256:9c1e...",
  "containerName": "atrium-app-jellyfin",
  "ownerToken": "32 random bytes, hex",
  "runtime": "docker",
  "createdAt": "2026-09-22T18:31:04Z" }
```

At creation Agent generates the owner token, chooses the container name
(`atrium-app-<appId>`), and stamps `io.atrium.managed=true`,
`io.atrium.app=<appId>` and `io.atrium.owner-token=<token>`.

Before **any** mutation, Agent requires, in order:

1. an entry for the requested `appId` in its own registry — absent means
   `not_managed`;
2. the live container fetched **by the id in that entry**, never by an id from the
   request;
3. `io.atrium.owner-token` equal to the recorded token, compared in constant time,
   and `io.atrium.app` equal to the requested app;
4. the container's creation identity still matching the record, so a recreated or
   id-reused container is not silently inherited.

Any mismatch is refused as `ownership_mismatch`, journaled by Agent, and marks the
app `disowned`. Agent never repairs this state on its own.

Registry entries are created **only** by `AppApply`, and `AppApply` refuses if a
container with the target name already exists. Core therefore has no path to
registering something it did not cause Agent to create.

### 5. Unmanaged containers are read-only, enforced by Agent

Containers Atrium did not create are the user's (A-16). `ContainerInventory`
returns them sanitized — id, name, image reference, state, published ports, and
nothing else; no environment, no labels beyond the managed marker, no command
line, bounded in count, as the prototype's inventory exporter already does.

**There is no operation, of any kind, that mutates an unmanaged container.** Not
start, not stop, not restart. The restriction is structural: mutation operations
take an `AppId`, and an unmanaged container has none.

A future adoption workflow is the only way this changes. It is owner-initiated, out
of Alpha scope, and will require a confirmation Agent can verify without trusting
Core — see ADR-014.

### 6. The trust property

> An attacker with arbitrary code execution as `atrium`, able to issue any
> sequence the Core-to-Agent protocol can express, cannot open the runtime socket,
> cannot express a runtime-native call, and cannot mutate any container that Agent
> did not itself create and record.

Forging ownership requires both the owner token, which Agent never releases to
Core or to any API response, and a registry entry, which Core cannot write.

Enforcement is tested: structurally at M1 (group membership, socket access, the
shape of the operation table, the absence of a proxy) and behaviourally at M2
(mutation refusal for unmanaged and for token-mismatched containers). See
[`../ROADMAP.md`](../ROADMAP.md).

## Consequences

**Good**

- The contradiction is gone. "Core is unprivileged" is now true without an
  asterisk, and the claim in `SECURITY.md` §4 is enforceable.
- The user's own containers are protected by a component that does not take
  instructions about ownership from anywhere.
- The runtime adapter is in one place, behind a protocol, which makes the Podman
  adapter a change in Agent rather than a change everywhere.
- Rootless containers stop being the thing that saves the architecture and become
  what they should be: a reduction of Agent's blast radius.

**Costs**

- **Every container read crosses the process boundary.** Inventory, logs and
  health all become Agent operations with serialization cost. Log streaming in
  particular needs a proper streaming response rather than a request/response pair,
  which is real work.
- Agent grows. It now contains a runtime client, the ownership registry and the
  spec builder — and Agent's size is itself a security property (`SECURITY.md`
  §19). This is the main price of the decision and it is paid deliberately.
- Two components must agree on the app lifecycle, so the protocol needs its own
  versioning and a defined behaviour on mismatch.
- Debugging is harder: a runtime problem is now one process removed from the
  developer, and Agent's diagnostics have to be good enough to compensate.
- An out-of-band `docker rm` of a managed container leaves a registry entry whose
  container is gone. Agent reports `disowned` and waits for the owner rather than
  guessing.

## Alternatives considered

**Core in the `docker` group** — the previous design. Rejected: root-equivalent,
and the reason this ADR exists.

**A socket proxy in front of the daemon** (docker-socket-proxy or equivalent).
Rejected on a dilemma: a proxy permissive enough to create containers with volumes
and networks is a Docker API passthrough with extra steps — `create` with a bind
mount of `/` is still available — and a proxy narrow enough to be safe is exactly
the typed operation protocol, which is what was built instead. A filtering proxy
also puts the security boundary in a component that parses the runtime's wire
format, which is a worse place for it than a typed enum.

**Rootless Podman in Alpha, with Core holding the rootless socket.** Genuinely
attractive: a rootless socket is not root-equivalent. Rejected for Alpha because
Docker is what the supported machines actually run, rootless brings its own
problems (user namespaces, storage drivers, port binding below 1024, cgroups
delegation), and the boundary would have to be redesigned again when a rootful
runtime appears. It remains the V1 end-state — inside Agent, where it shrinks
Agent's exposure rather than justifying Core's.

**Core creates containers, Agent only does the privileged parts.** Rejected: there
is no such split. Creating a container *is* the privileged part.

**Trusting a `io.atrium.managed=true` label alone.** Rejected: labels are writable
by anyone who can reach the runtime, so the check would be satisfied by anything an
attacker could label — and by a user's own container that happened to carry the
label. Evidence has to be something Core cannot write.
