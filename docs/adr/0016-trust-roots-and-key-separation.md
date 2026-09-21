# ADR-016 — Three trust roots, kept separate

**Status:** accepted
**Date:** 2026-09-22
**Related:** ADR-003, ADR-005, ADR-013, ADR-014
**Assumptions:** A-28

## Context

ADR-014 says Agent verifies the catalogue signature "with its own key". Reviewed
literally, that phrase is wrong in a way that matters: it reads as though the
server's device identity — the key pair clients pin during pairing — also
establishes who published the software. Those are different questions with
different answers, different lifetimes and different consequences when one is
lost.

Conflating them would be a real defect, not a wording problem. A server's identity
key is generated on the machine, exists in one copy, and is rotated by the owner.
A publisher key signs artefacts for every installation in existence and is held by
whoever produces releases. If a machine's identity key could vouch for software,
then compromising one home server would be enough to make it accept anything as
first-party; and if a publisher key were used for TLS, it would be handled online,
on every machine, in the component most exposed to the network.

## Decision

**Three keys. Three domains. No overlap, in either direction.**

| Key | Where it lives | Signs / proves | Must never |
| --- | --- | --- | --- |
| **Server device identity** | generated per server at first boot; private half at `/etc/atrium/tls.key`, `0600 atrium:atrium`, never leaves the machine | this server's TLS; the SPKI clients pin at pairing (ADR-003) | sign a release, a catalogue, a manifest, or anything another machine consumes |
| **Release signing** | offline, publisher side; public half distributed with the install script and with the package repository | the tarball, `.deb`/`.rpm` and updates — **including the Agent binary itself** | verify catalogue content, or take part in TLS |
| **Catalogue publisher** | offline, publisher side; public half **compiled into the Agent binary** | the catalogue index, and through it every manifest | take any TLS or pairing role |

Both publisher keys are Ed25519: one algorithm, no parameter choices, no
certificate chain to validate.

### The Alpha mechanism, in full

This is the minimum that is actually safe. There is no PKI, no certificate
hierarchy and no marketplace infrastructure, because none of that is needed while
the catalogue is first-party and ships with the release.

**The catalogue is a directory:** manifests, their inert assets, an `index.json`
listing `{ id, version, revision, manifestPath, manifestSha256 }` for every entry,
and `index.json.sig`, a detached Ed25519 signature over the canonical bytes of
`index.json`.

**Verification is two steps and one signature:**

1. Agent verifies `index.json.sig` over `index.json` using a **compiled-in
   catalogue publisher key**.
2. Agent hashes the manifest it is about to use and requires it to equal the
   `manifestSha256` recorded in the index it just verified.

Manifests are therefore covered by the index signature without each needing its
own, and a manifest substituted anywhere in the path — including by a compromised
Core — fails step 2.

**Agent holds an ordered set of accepted publisher keys**, compiled in: the
current key, optionally a `next` key during an overlap window, and optionally the
`previous` key so that a staged upgrade with mixed component versions does not
fail closed for the wrong reason. Each carries an identifier and a not-after date.

**Core cannot touch the trust root.** No operation parameter carries a key, a key
identifier, a key path, a trust-root file, or a flag that skips or relaxes
verification. `AppApply` carries manifest bytes, the index entry and the index
signature — data to be verified, never the means of verifying it. There is no API
route, no configuration value readable by Core, and no environment variable that
changes which keys Agent accepts.

**Unsigned or incorrectly signed content is refused.** `agent.catalogue_signature_invalid`
when the index signature does not verify, `agent.manifest_digest_mismatch` when
the manifest does not match the verified index. Neither has an override reachable
from the API.

**The development path is explicit and visible.** An operator working on a
manifest can install an additional public key into
`/etc/atrium/agent/publisher-keys.d/` (directory `0755 root:root`, files `0644`,
read only by Agent), or enable unsigned local manifests, both **only at the server
console**. The directory is empty by default. Whenever it is non-empty or the
unsigned path is enabled, Agent reports `catalogueTrust: "extended"` through the
capability surface, and the interface shows it. Reduced trust is a state the owner
can see, never a silent setting.

### Rotation

**Alpha: rotation is a release.** The catalogue publisher key is compiled into
Agent, and the Agent binary is itself verified with the release signing key at
install and update time. Publishing a new Agent publishes a new trusted key set.
That is the entire mechanism, and while the catalogue is first-party and bundled
it is sufficient.

The honest consequence: **the catalogue key's integrity reduces to the release
key's**, and emergency revocation of a catalogue key requires shipping a release.
Both are limitations, both are stated, neither is hidden behind an implication of
independence that does not exist in Alpha.

**Before a remote catalogue or any third-party publisher exists**, a real
trust-root rotation is required, and the shape is recorded now so it is not
improvised later: a **signed key-transition statement** — a short document naming
the incoming key and its validity window, signed by the outgoing key and, during
an overlap, counter-signed by the incoming one — which Agent verifies and then
persists to its root-only store, so rotation no longer requires a new binary. That
is deliberately the smallest thing that works. It is not TUF, it is not a
certificate authority, and it is not built now.

## Consequences

**Good**

- The question "who published this software" and the question "which machine am I
  talking to" have separate answers, separate keys and separate failure modes.
- Compromising one server yields that server. It does not yield the ability to
  make any Atrium installation accept software as first-party.
- The publisher keys stay offline. Neither is ever loaded by a network-facing
  process.
- Compiling the catalogue key into Agent means the verifier and its trust root
  ship and are replaced together — there is no window where a new binary trusts an
  old key file an attacker left behind.
- The mechanism fits on one page and needs no infrastructure to stand up.

**Costs**

- **Rotating the catalogue key requires a release.** Fine at this scale, and
  unacceptable the moment a remote catalogue exists — which is why the transition
  statement is designed now rather than discovered then.
- **Two publisher keys to hold, protect and eventually rotate**, with the
  operational discipline that implies, for a project that currently has one
  maintainer. If they are ever stored together or used from the same machine, the
  separation is decorative; that is a process risk this document cannot solve.
- Agent carries signature verification and a key set, so it grows again. The same
  cost noted in ADR-013 and ADR-014, paid for the same reason.
- The `publisher-keys.d` escape hatch exists, and any escape hatch is a thing to
  test, document and watch. It is mitigated by being console-only and by being
  visible in the interface when used — not by being secret.

## Alternatives considered

**One key for everything.** The thing this ADR exists to reject. It would make a
single server compromise a supply-chain compromise.

**Re-use the release signing key for the catalogue.** Tempting — one fewer key to
hold — and rejected because the two have genuinely different cadences: catalogue
entries change whenever an upstream image is re-pinned, releases change far less
often, and an operational mistake with the busier key should not compromise the
quieter one.

**Per-server trust: the owner registers a catalogue key at install.** Rejected for
Alpha: it makes every owner a trust administrator to solve a problem first-party
signing already solves, and the first thing most people would do is paste whatever
the installer printed.

**A certificate authority, or TUF.** Rejected as unnecessary infrastructure at this
stage. Both become relevant if third-party publishers ever exist; neither is
warranted for a bundled first-party catalogue, and building one now would be
designing a marketplace PKI by accident.

**Trust the container registry's own signing (cosign, Notary).** Useful later and
orthogonal: it says something about the image, not about Atrium's manifest, the
permissions it requests, or the digest pinning. It does not replace this.
