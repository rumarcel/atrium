# ADR-014 — Agent derives and constrains every container specification

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-005, ADR-013, ADR-004, ADR-016
**Assumptions:** A-13, A-14, A-28

## Context

ADR-013 moves the runtime connection into Agent. That closes the socket, but it
does not by itself close the escalation path, because **creating a container is a
root-equivalent primitive when the specification is unconstrained.** A container
with `privileged: true`, or `--pid=host`, or a bind mount of `/`, or
`CAP_SYS_ADMIN`, or `/dev` mapped in, is root on the host. If Core could hand Agent
a specification and have it applied, then moving the socket would have changed
nothing except the number of hops.

ADR-005 also assumed that a manifest's declared permissions would be shown to the
owner and consented to in the UI. A compromised Core renders that consent screen,
records the consent, and reports it to Agent. Consent that only Core observes is
consent an attacker can manufacture.

There is a third gap. If Core chooses the image, a compromised Core chooses to run
anything it likes — confined, but still arbitrary code on the machine.

## Decision

### 1. Agent derives the specification; Core's version is input, not instruction

`AppApply` carries the **signed manifest**, the user's validated settings, the
allocations Core computed (ports, volume names) and the secret values. Agent then:

1. verifies the catalogue index signature against a **catalogue publisher trust
   root that Core cannot supply, name or replace** — a key compiled into Agent,
   entirely separate from the server's device identity key (ADR-016), then
   confirms that the manifest hashes to the digest recorded in that verified
   index;
2. checks that the manifest's digest-pinned images match what the signature covers;
3. **re-derives the container specification from the manifest**, using its own spec
   builder;
4. accepts Core's allocations only as values inside fields the manifest already
   declares — a port number, a generated secret, a chosen timezone — never as new
   fields, new mounts, or new capabilities;
5. applies the constraint set in §2 and refuses the whole operation on any
   violation.

A specification composed by Core is never applied verbatim. There is no operation
that accepts one.

### 2. The Alpha constraint set

Agent refuses any derived specification that would include:

- `privileged`
- any added kernel capability; the spec drops all capabilities and sets
  `no-new-privileges`
- host networking, host PID, host IPC, or host UTS namespaces
- any device mapping
- any bind mount of a host path
- any volume source other than an Agent-created named volume for that app, or a
  path under an **Agent-registered data root**
- any image reference without a digest, or a digest not covered by the verified
  signature
- any container name outside `atrium-app-<appId>`
- any label in the `io.atrium.*` namespace other than the three Agent stamps

Data roots are registered with Agent out of band — at install time, or later from
the server console — not through the API. An app that needs the owner's media
folder gets it because the owner told Agent where the media folder is, once, on the
machine; not because a manifest asked and Core said the owner agreed.

### 3. Capabilities beyond the constraint set need a grant Agent can verify

Hardware transcoding (`/dev/dri`), host networking and similar legitimate needs are
**not available in Alpha**. The manifest schema does not accept them and Agent
refuses them.

They return through a grant mechanism whose authority does not pass through Core:
an owner action at the server console (`atriumctl grant <app> <capability>`) that
writes to Agent's own store, is displayed in the UI as a granted capability, and is
revocable the same way. The design of that mechanism is deferred; what is frozen
here is that **a capability grant is never a field in an API request.**

### 4. Consequences for the manifest

`APP-SDK.md`'s `permissions` block stays in the schema as the place these are
declared and displayed, but in Alpha every field in it must be empty or false, and
a manifest that populates one fails validation at catalogue build time and again at
`AppApply`. The block exists so that the concept is designed; it does not yet
grant anything.

## Consequences

**Good**

- The escalation path from "compromised Core" to "root" is closed at the point
  where it actually exists: specification construction.
- The catalogue signature becomes a real control rather than a formality, because
  the component that enforces it is not the component an attacker is assumed to
  hold, and the key it enforces against is one Core cannot reach (ADR-016).
- A compromised Core cannot run an arbitrary image, only catalogue software, and
  only in a confined container.
- The rule is simple enough to test: enumerate the spec builder's output for every
  catalogue manifest and assert the constraint set.

**Costs**

- **Alpha cannot ship hardware transcoding**, which is the single most-wanted
  feature of a home media server (A-14). Jellyfin will transcode on the CPU, and
  on an old laptop that is slow. This is a real, user-visible cost of the security
  position, and it is accepted rather than worked around.
- Manifest expressiveness is reduced to what the constraint set allows, so some
  applications simply cannot be catalogued yet.
- Agent now contains the spec builder, the signature verifier and the catalogue
  public key. Agent grows again; see ADR-013's cost note.
- The same derivation logic effectively exists twice — Core needs to *predict* the
  spec to show the user ports and volumes before install, Agent *decides* it. They
  must not drift. Mitigation: one shared derivation crate, with Agent's call being
  the authoritative one and a test asserting both produce identical output for
  every catalogue manifest.
- Registering data roots out of band is friction for the owner, and it is friction
  in exactly the place — "where is my media?" — that a first-run wizard would
  rather smooth over.

## Alternatives considered

**Core builds the spec, Agent validates it against the manifest.** Nearly
equivalent, and tempting because it keeps the builder in the easier place to
change. Rejected: validation of a rich structure against a schema is a much larger
attack surface than deriving that structure from a small input, and every gap in
the validator is an escalation. Deriving is strictly safer than checking.

**Allow declared capabilities with UI consent, as originally written in ADR-005.**
Rejected: Core renders the consent, so a compromised Core manufactures it.

**Allow host paths if they are inside an allowlist Core sends.** Rejected for the
same reason — the allowlist would come from the component under attack.

**Let Agent ask the user directly.** Agent has no user interface and no way to
authenticate a person; giving it one would mean a second authentication system in
the most security-sensitive component. Rejected in favour of the console grant,
where the authentication is physical access to the machine.

**Ship hardware transcoding in Alpha behind a flag.** Rejected: a flag that
disables the constraint set is the constraint set not existing. If transcoding
matters more than the property, that is a product decision to take explicitly,
with a superseding ADR — not a default that quietly weakens the boundary.
