# ADR-012 — SQLite state store with forward-only migrations

**Status:** accepted; the `secrets.key` mode is superseded by [ADR-017](0017-install-time-identity-and-root-owned-key-material.md)
**Date:** 2026-09-22
**Related:** ADR-001, ADR-007
**Assumptions:** A-26

## Context

Core needs to keep devices, users, roles, installed apps and their settings,
allocated ports and volumes, secrets, operations, audit records, notifications,
external links and metric history — durably, across restarts, upgrades and power
cuts, on hardware that may be an SD card in a Raspberry Pi.

The prototype stores everything as versioned JSON documents per concern, written
atomically through a same-directory temporary file and an atomic replace, with the
previous valid copy kept as a `.bak` and a visible recovery notice when the saved
file is invalid. That design is careful and correct for a handful of documents
owned by one process. It does not extend to relational data with foreign keys,
concurrent writers, queries, or an audit log that must not be rewritten to append
to.

## Decision

**SQLite, in WAL mode, with `synchronous=NORMAL`**, as Core's state store. Three
databases, separated by lifetime and sensitivity:

| Database | Contents | Notes |
| --- | --- | --- |
| `/var/lib/atrium/atrium.db` | devices, users, apps, settings, links, operations, audit, notifications | the product's state |
| `/var/lib/atrium/secrets.db` | app secrets and third-party credentials | encrypted with a key at `/etc/atrium/secrets.key` (`0600`; superseded: `root:atrium 0640`, created at installation — ADR-017); separable, separately backed up, separately destroyable |
| `/var/lib/atrium/metrics.db` | metric history | fixed-size ring, disposable, excluded from backups, never blocks a write to the others |

Bootstrap configuration that cannot come from a database — listen address, data
directory, log level — stays in `/etc/atrium/core.toml`. Everything else is state,
not a file. **There is no hand-edited YAML that Core reads at runtime.**

### Migrations

- Numbered, forward-only, embedded in the binary, each in a single transaction.
- `user_version` records the applied schema.
- **The database file is copied before the first migration of an upgrade.** The
  copy is kept until the new version has started successfully once.
- A failed migration rolls back its transaction, leaves the previous database
  intact, and starts Core in **recovery mode**: `/healthz`, diagnostics and the
  restore path work; nothing else does. Core does not crash-loop and does not
  start against a half-migrated schema.
- A downgrade is not supported. Rolling back a release restores the pre-migration
  copy, which is why the copy exists.
- Migrations are tested against fixtures from every previously released schema,
  not only against an empty database.

### Durability rules

- Every operation's intent is committed before the work begins, so a crash
  mid-operation is reconstructible — the prototype's rule, now enforced by a
  transaction.
- Audit records are append-only. No code path updates or deletes one; retention
  trims the oldest by count and age.
- Backups of `atrium.db` use SQLite's backup API or `VACUUM INTO`, never a file
  copy of a live database.
- Metric history is bounded by row count at write time, not by a periodic cleanup
  that might not run.

## Consequences

**Good**

- Relational integrity for data that is genuinely relational: an app's ports,
  volumes and secrets cannot outlive the app.
- Transactions give the "durable intent before privileged action" rule a real
  mechanism instead of a convention.
- One file to back up, one file to restore, one file to inspect with standard
  tools when something is badly wrong.
- Append-only audit is enforceable at the schema and code-review level.
- No server process, no port, no connection pool, no second thing to supervise.
- Separating secrets means a diagnostics bundle or a settings export can be built
  from `atrium.db` without touching them.

**Costs**

- SQLite on an SD card with WAL will eventually meet a card that lies about
  flushing. `PRAGMA synchronous=NORMAL` is a deliberate trade of a small
  durability window for far less write amplification; the alternative is
  `FULL` and a slower, shorter-lived card. Revisit with real Raspberry Pi data.
- Write concurrency is a single writer. Core must not hold a write transaction
  across an operation that waits on the network or on Agent, or the whole product
  stalls. This is a real footgun and needs a lint or a review habit.
- Migrations become the riskiest part of every upgrade, and the pre-migration copy
  needs enough free disk to exist — which is not guaranteed on a full server, so
  the upgrade must check space first and refuse rather than half-migrate.
- Three database files is three things to keep consistent during backup and
  restore.

**Neutral**

- The prototype's per-document JSON files stay in the desktop client, where they
  are appropriate. The two storage models do not need to converge.

## Alternatives considered

**Keep versioned JSON documents, as the prototype does.** Rejected: no
relationships, no transactions, whole-file rewrites for a single field, an audit
log that must be rewritten to append to, and no query capability. It is the right
design at ten documents and the wrong one at ten thousand rows.

**PostgreSQL.** Rejected: a second service to install, supervise, back up, upgrade
and secure, on a machine that may have 2 GB of RAM, for a workload that never
exceeds one writer.

**An embedded key-value store (sled, redb, LMDB).** Rejected: relationships and
ad-hoc queries would be written by hand, and none of them has SQLite's inspection
tooling, which matters most precisely when the product is broken.

**One database instead of three.** Rejected: secrets and disposable metric history
have different backup, retention and destruction rules from the product's state,
and separating them by file is simpler than separating them by policy.

**Configuration files as the source of truth, with the database as a cache.**
Rejected: two sources of truth, and it invites hand-editing, which invites a
support burden the product cannot carry.
