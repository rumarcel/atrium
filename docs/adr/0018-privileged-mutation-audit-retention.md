# ADR-018 — Privileged mutation history is retained apart from everything else, or the mutation does not happen

**Status:** accepted. The requirement binds the first mutating Agent
operation (M2); nothing is implemented by this ADR
**Date:** 2026-09-24
**Related:** ADR-001, ADR-004 (constraint 8, *durable intent before
dispatch*, and 9, *audited twice*), ADR-013, ADR-014
**Supersedes:** nothing

## Context

M1C built Agent's journal. It writes one line per connection, whatever the
outcome, into a root-only directory the `atrium` user cannot read or change,
and it returns a result only after the line is on disk. Retention is by size:
five files of 8 MiB, oldest discarded (plan §3.6).

That is adequate for M1, because every M1 operation is read-only. It is not
adequate once Agent can change the machine. The M1C security review stated
the weakness plainly (plan §17, M1C *As built*):

- A compromised Core can make Agent write journal lines — read-only calls,
  malformed frames, rejected connections — up to the rate limit, and so push
  old lines out through rotation. At 8 lines a second, the whole 40 MiB is
  replaced in about seven hours.
- In M1 the history lost that way is a history of reads. In M2 it would be a
  history of container starts, stops, installs and removals, which is exactly
  the evidence ADR-001 promises a compromised Core cannot erase.

Size-bounded, shared retention therefore lets an attacker **erase privileged
history without touching a file**, simply by generating noise. The architecture
promises the opposite.

## Decision

Before the **first mutating Agent operation** is implemented, Agent's audit
evidence for privileged mutations must meet all of the following. They are
requirements on whatever design M2 chooses, not a design.

1. **A separate lane.** Privileged mutation events are retained in a security
   or mutation lane, or with equivalent isolation, apart from read-only
   probes, rejected connections and connection noise. Nothing that does not
   mutate may consume the mutation lane's capacity or evict an entry from it.
2. **A retention floor of 30 days.** A mutation entry younger than 30 days is
   never discarded to satisfy a size cap. Older entries may be discarded under
   a policy stated in the implementing design.
3. **Fail closed on capacity.** If durable audit capacity is exhausted and
   space cannot be made without discarding an entry inside the floor, Agent
   **refuses new mutating operations**, with a stable error code, until
   capacity returns. Read-only operations may continue.
4. **Record before act.** An operation never executes first and discovers
   afterwards that its required security record could not be written. The
   intent record is durable before anything privileged happens, as ADR-004
   constraint 8 already requires. The outcome record follows. If the outcome
   record cannot be written, that failure is itself recoverable from the intent
   record, and the operation is `unknown`, never silently `ok`.
5. **Out of Core's reach.** Core cannot erase, truncate, rewrite, reorder or
   forge mutation history, directly or through any Agent operation. As in M1,
   this rests on root-only ownership, and no operation may take a parameter
   that addresses the journal.

**This deliberately chooses denial of service over the silent destruction of
privileged audit evidence.** A server that cannot record what it is about to do
stops doing privileged things. It never does them unrecorded.

### What this ADR does not decide

- The storage quota, file layout, or whether the lane is a second directory, a
  second file set, an append-only database or a copy to the system journal.
  Those belong to the M2 design, which must show how requirements 1–5 hold.
- Retention for read-only history. The M1 journal may stay bounded by size as
  built, because M1's operations are read-only.

### Where it is checked

The first review of an M2 mutating operation must show, with tests, that
connection noise and read-only calls cannot evict a mutation entry younger than
30 days; that exhausted capacity refuses a mutation before it runs; and that
Core, as `atrium`, cannot alter the lane.

## Consequences

**Good**

- The Core-compromise claim in `SECURITY.md` §4 stays true when Agent starts
  changing the machine. Noise cannot buy an attacker deniability.
- The trade-off is written down before anyone is tempted to "just raise the
  cap" or to drop the oldest entry to keep an operation running.

**Costs**

- A full disk, or an attacker filling it, can stop app management until space
  is recovered. This is accepted. It is visible, recoverable and audited,
  whereas lost evidence is none of those things.
- M2 carries the design and test cost of a second retention lane.

## Alternatives considered

**Keep one size-bounded journal and raise the cap.** Rejected: any fixed cap is
a number of noise lines an attacker can generate.

**Rate-limit harder and keep one journal.** Rejected as the mechanism: a rate
limit bounds how fast history can be displaced, not whether it can be. It stays
as defence in depth.

**Drop the oldest mutation entry when full, and keep operating.** Rejected:
this is exactly the silent destruction this ADR exists to forbid.

**Send mutation history off the machine.** Not rejected, but not assumed:
Atrium is local-first (ADR-006), and remote copies can be an addition, never
the only record.
