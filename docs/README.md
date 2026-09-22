# Atrium documentation

Two things live here, and they describe different products.

**The prototype** is the Windows desktop application in this repository today: a
single-user control centre for a home server someone else already built. It works,
it is documented in the root [`README.md`](../README.md), and its phase history is
in the root [`ROADMAP.md`](../ROADMAP.md).

**The platform** is what Atrium is becoming: a server-side product that turns any
computer into a personal cloud. Everything in this directory except
[`windows-release.md`](windows-release.md) describes that target. None of it is
implemented yet.

[`PROTOTYPE-ASSESSMENT.md`](PROTOTYPE-ASSESSMENT.md) is the bridge — what exists,
what it costs, and what carries over.

**Revised 2026-09-22 by a hardening review.** The first draft let Core join the
`docker` group, which is root-equivalent and contradicted the trust boundary the
whole Core/Agent split exists to provide. The container runtime now lives behind
Agent (ADR-013), Agent derives and constrains every container specification
(ADR-014), and the claim about what a Core compromise yields is stated as an
enforceable, tested property in [`SECURITY.md`](SECURITY.md#4-the-core-compromise-property)
rather than as a hope. The same review corrected a byte-payload privileged
operation, respecified the pairing exchange on standard constructions, and
downgraded "static musl binaries" from architecture to packaging.

A follow-up pass fixed the pairing secret's arithmetic (16 bytes, 128 bits, 26
Crockford Base32 symbols, with normalization and canonicality specified), separated
the three trust roots that the phrase "Agent's own key" had blurred together
(ADR-016), and corrected the rationale for requiring TLS 1.3.

## Read in this order

| Document | Answers |
| --- | --- |
| [`PRODUCT.md`](PRODUCT.md) | What is being built, for whom, what is deliberately not being built |
| [`PROTOTYPE-ASSESSMENT.md`](PROTOTYPE-ASSESSMENT.md) | What is in the repository now and what it means for the plan |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Components, boundaries, flows, identity, persistence, failure handling |
| [`SECURITY.md`](SECURITY.md) | Threat model, trust boundaries, and the rules that outrank features |
| [`API.md`](API.md) | The contract every client speaks |
| [`APP-SDK.md`](APP-SDK.md) | How applications are described and installed |
| [`UX-STATES.md`](UX-STATES.md) | The states every surface must implement |
| [`PLATFORM-MATRIX.md`](PLATFORM-MATRIX.md) | What is supported, planned, experimental, unsupported |
| [`ROADMAP.md`](ROADMAP.md) | Stages, and the acceptance criteria for the first milestone |
| [`M1-IMPLEMENTATION-PLAN.md`](M1-IMPLEMENTATION-PLAN.md) | How M1 gets built: layout, protocol, state, pairing, providers, units, installer, passes |
| [`M1-TEST-PLAN.md`](M1-TEST-PLAN.md) | How M1 gets proved, and which tests need a real machine |
| [`ASSUMPTIONS.md`](ASSUMPTIONS.md) | What the design believes but has not verified |
| [`CHECKPOINT-M1A.md`](CHECKPOINT-M1A.md) | **Read this first if you are resuming.** Where the work stopped and what comes next |
| [`adr/`](adr/README.md) | The decisions that are frozen, and what they cost |

Sixteen ADRs. The three added by the hardening review —
[013](adr/0013-container-runtime-ownership.md),
[014](adr/0014-agent-enforced-container-specs.md) and
[015](adr/0015-canary-coexistence-with-the-prototype.md) — are the ones to read
first if you only read three, followed by
[016](adr/0016-trust-roots-and-key-separation.md) on key separation.

[`windows-release.md`](windows-release.md) is operational documentation for the
existing desktop application and is unaffected by any of the above.

## How to use these documents

- **A decision that contradicts an ADR needs a superseding ADR**, not a code
  comment. ADRs are not edited to change their direction.
- **`SECURITY.md` is normative.** A feature that cannot be built inside its
  boundaries is not built until the boundary is moved there first.
- **`UX-STATES.md` is a checklist.** A surface that only renders success is
  unfinished.
- **Unverified beliefs go in `ASSUMPTIONS.md` with an identifier**, and the
  document that depends on one cites it. An assumption that turns out to be wrong
  is a traceable design change, not a surprise.
- **The M1 documents are implementation planning, not architecture.** They must
  not contradict an ADR; where a criterion forced a larger design than it looked,
  the plan says so in its own section rather than adjusting the criterion.
- Nothing here promises a date.
