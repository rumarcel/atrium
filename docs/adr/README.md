# Architecture decision records

Decisions that are expensive to reverse, recorded when they are made rather than
reconstructed afterwards. An ADR is not a design document: it states the context,
the decision, what it costs, and what was rejected.

**Status values:** `proposed` · `accepted` · `superseded by ADR-nnn` ·
`deprecated`. An accepted ADR is not edited to change its decision — it is
superseded by a new one.

Every ADR here is `accepted` as of the productization documentation pass, meaning
it is frozen for the Alpha work. Implementation may reveal that one is wrong; the
response is a superseding ADR, not a quiet change of direction.

**Hardening review, 2026-09-22.** This set was written as one proposal and
reviewed before any of it was implemented or committed, so the records marked
*revised* below were corrected in place rather than superseded — nothing had yet
been built on the version that was wrong. That latitude ends here: from the first
commit of implementation, a decision changes only by superseding ADR. The review
found that Core's membership of the `docker` group contradicted the Core/Agent
trust boundary (ADR-013, ADR-014), that a byte-payload operation in ADR-004 was a
root-execution primitive, that ADR-003's pairing construction used a hand-rolled
channel binding and left the code's entropy unstated, and that ADR-007 froze a
packaging choice as though it were architecture.

A second review pass corrected three things the first one left wrong: the pairing
secret was specified as "32 bytes rendered as 26 Crockford base32 characters",
which is arithmetically impossible (ADR-003 now fixes 16 bytes / 128 bits / 26
symbols, with encoding and normalization written out); the catalogue was said to be
verified with Agent's "own key", which reads as conflating a server's device
identity with software-publisher trust (ADR-016 separates three keys); and TLS 1.3
was justified as necessary for exporters, which is not true (ADR-003 §4 now states
it as a deliberate baseline instead).

A third pass, during M1 planning, asked three questions the earlier reviews had
not. Does the exporter binding strand the future browser client? — it would have,
so the transcript now names a **binding profile** and the browser path is
additive rather than a replacement (ADR-003 §4a). Is Argon2id right for a
256-bit random device token? — no, and it is superseded by `SHA-256` with the
reasoning recorded (ADR-003 §7). Can the running Core replace its own identity
key? — it could, and now cannot: the key is `root:atrium 0640` in a root-owned
directory, enforced by ownership rather than by a unit file. The same pass
removed a proposed recovery authentication mirror in favour of console-only
restore, and amended two M1 acceptance criteria to match — recorded in
[`../ROADMAP.md`](../ROADMAP.md#criteria-amended-during-planning).

**Supersession, 2026-09-24.** M1B built and proved the identity ownership
that M1 planning had decided — keys created once at installation,
`root:atrium 0640` in a `root:atrium 0750` directory — while ADR-003 §1,
ADR-012 and ADR-016 still described first-boot generation and `0600` files
owned by the service. [ADR-017](0017-install-time-identity-and-root-owned-key-material.md)
supersedes exactly those sentences. The older ADRs carry a note and are
otherwise unchanged.

**Supersession, 2026-09-24 (M1E).** ADR-003 §3 said the armed pairing secret
is kept "only [as] a hash", but §4 derives the proof key from the secret, so
a hash cannot verify a proof. [ADR-019](0019-sealed-pairing-secret.md)
replaces that one storage rule with sealed storage under `secrets.key`; the
rest of ADR-003 stands.

| ADR | Decision | Status |
| --- | --- | --- |
| [ADR-001](0001-core-and-agent-separation.md) | Core and Agent separation | accepted, revised |
| [ADR-002](0002-provider-adapter-architecture.md) | Provider/adapter architecture with runtime capability negotiation | accepted, revised |
| [ADR-003](0003-authentication-and-device-pairing.md) | Server identity, SPKI pinning, binding-profile pairing, SHA-256 device-token verifier | accepted, revised ×2; §1 in part superseded by ADR-017, §3's secret storage by ADR-019, the five-failure lock by ADR-020 |
| [ADR-004](0004-privileged-operation-model.md) | Closed typed privileged operation set, no shell, no sudo, no byte payloads | accepted, revised |
| [ADR-005](0005-application-platform.md) | Declarative app manifests as the application platform | accepted, revised |
| [ADR-006](0006-local-first-and-remote-access.md) | Local-first with optional, tiered remote access | accepted |
| [ADR-007](0007-core-language-and-distribution.md) | Rust for Core and Agent, self-contained binaries; linkage is packaging | accepted, revised |
| [ADR-008](0008-api-shape-and-versioning.md) | HTTP/JSON API, operation resources, additive `v1` | accepted |
| [ADR-009](0009-clients-are-api-clients.md) | Every client is an unprivileged API client | accepted |
| [ADR-010](0010-prototype-disposition.md) | What the prototype contributes, keeps and retires | accepted |
| [ADR-011](0011-identifier-migration.md) | `personal-hub` → `atrium` identifier migration; reverse-DNS stays provisional | accepted, revised |
| [ADR-012](0012-state-store-and-migrations.md) | SQLite state store with forward-only migrations | accepted; `secrets.key` mode superseded by ADR-017 |
| [ADR-013](0013-container-runtime-ownership.md) | The container runtime belongs to the Agent | accepted |
| [ADR-014](0014-agent-enforced-container-specs.md) | Agent derives and constrains every container specification | accepted |
| [ADR-015](0015-canary-coexistence-with-the-prototype.md) | Alpha runs as a canary beside the prototype | accepted |
| [ADR-016](0016-trust-roots-and-key-separation.md) | Three trust roots — device identity, release signing, catalogue publisher — kept separate | accepted; identity key location superseded by ADR-017 |
| [ADR-017](0017-install-time-identity-and-root-owned-key-material.md) | Identity material is created at installation and owned by root (`root:atrium 0640` in `root:atrium 0750`); the running service never generates it | accepted |
| [ADR-018](0018-privileged-mutation-audit-retention.md) | Privileged mutation history gets its own retention lane with a 30-day floor; if it cannot be kept, new mutations are refused | accepted; binds the first mutating Agent operation (M2) |
| [ADR-019](0019-sealed-pairing-secret.md) | The armed pairing secret is stored as XChaCha20-Poly1305 ciphertext under `secrets.key`, bound to its arming, never as plaintext or a bare digest | accepted (M1E); lifetime row "fifth failed proof" removed by ADR-020 |
| [ADR-020](0020-no-failed-proof-lock.md) | A failed pairing proof consumes its attempt and nothing else; no five-failure lock | accepted (before M1F) |
