# ADR-011 — `personal-hub` → `atrium` identifier migration

**Status:** accepted — revised in the hardening review of 2026-09-22
**Date:** 2026-09-22
**Related:** ADR-010, ADR-012

## Context

The product was renamed from Personal Hub to Atrium. The user-visible names were
changed; three identifier namespaces were deliberately left alone because
changing them destroys user data:

| Identifier | Current value | Effect of a naive change |
| --- | --- | --- |
| Tauri identifier | `com.muhta.personal-hub` | the per-user configuration directory moves; every setting appears lost |
| Credential targets | `PersonalHub/credentials/v1/…`, `PersonalHub/server-control/v1/<ip>` | stored secrets become unreachable, and the old entries are orphaned in Credential Manager |
| Agent unit and account | `personal-hub-agent.service`, `personalhub-agent` | a server-side reinstall |

Beyond those three, the old name is spread through things that are merely
confusing: the crate name `personal-hub`, lib name `personal_hub_lib`, npm package
name, `personal-hub://` event names, the `PersonalHub/0.1` user agent, a thread
name, and the CI artefact `personal-hub-windows-unsigned`.

A public repository called `atrium` whose binary is `personal_hub_lib` and whose
identity is `com.muhta.personal-hub` is a product that looks unfinished, and the
`com.muhta.` prefix ties the identifier to one person's username.

## Decision

Migrate everything to `atrium`, in three tiers, with data preserved.

### Tier 1 — cosmetic, no data impact. Do at any time.

Crate name → `atrium-desktop`, lib name → `atrium_desktop_lib`, npm package name →
`atrium`, thread names, user agents (`Atrium/<version>`), CI artefact names,
comments and documentation.

### Tier 2 — data-bearing, needs a migration step. Do once, deliberately.

**Tauri identifier** `com.muhta.personal-hub` → a reverse-DNS identifier that is
**provisionally** `app.atrium.desktop` and is **blocked on product ownership
supplying a domain**. A reverse-DNS namespace is a claim of ownership over a DNS
name; this project does not own `atrium.app` and will not act as though it does.
Until a domain exists, the current identifier stays in place — it is ugly, it
contains a personal username, and it is nonetheless harmless, whereas squatting a
namespace is not. Nothing else in this ADR depends on resolving it.

When a domain does exist and the identifier changes, the migration is already
designed: on first run under the new identifier, if the new configuration
directory is absent and the old one exists, copy every document across, validate
each one against its schema, write a marker recording the migration, and leave
the old directory untouched. A failed copy leaves the app in first-run state with
a recovery notice naming the old path — never a silent empty configuration.

**Credential targets** `PersonalHub/…` → `Atrium/…`.
Lazy migration at the point of use: on read, try the new target; on miss, try the
old one, and if found, write it to the new target with the same origin binding and
then delete the old. A failed delete is journaled and retried, reusing the
prototype's existing credential-cleanup journal machinery. Nothing is ever deleted
before its replacement is confirmed written.

**Event names** `personal-hub://…` → `atrium://…`. Emit both for one release so a
mixed-version frontend and backend cannot deadlock during an upgrade; drop the old
names the release after.

### Tier 3 — server side. Do as part of the platform, not before.

The agent's unit names and account (`personal-hub-agent.service`,
`personalhub-agent`) are not migrated at all. The Python agent is retired by
ADR-010 and replaced by `atrium-agent.service` running as `atrium`. The
documented uninstall for the old agent removes its unit, account, sudoers entry
and state directory; there is no in-place rename.

### Provisional identifiers

Three namespaces conventionally derive from a DNS name the project owns. None are
blocking, and all are recorded as provisional until product ownership supplies a
domain:

| Identifier | Provisional value | Why it is provisional |
| --- | --- | --- |
| Desktop/app identifier | `app.atrium.desktop` | reverse-DNS; a direct ownership claim |
| Container label namespace | `io.atrium.*` | reverse-DNS by convention for OCI labels |
| mDNS service type | `_atrium._tcp` | a DNS-SD service type; ideally registered with IANA |

They are used in documentation and prototypes so the design can be written down.
Shipping any of them publicly is gated on the domain question, and a change of
prefix afterwards is a find-and-replace in Atrium's own code, not a data
migration — with one exception: container labels are stamped onto live
containers, so changing that namespace after Alpha would need the same
lazy-migration treatment as the credential targets above, applied by Agent.

### Rules

- Every tier-2 migration is idempotent, logged, and produces a visible notice in
  Settings when it runs or when it fails.
- No migration deletes anything before its replacement is verified present.
- The migration marker records the source version, so a later migration can tell
  what it is dealing with.
- Tier 2 is not attempted during the platform build-out; it lands as its own
  change with its own tests.

## Consequences

**Good**

- The repository, the binary, the process, the identifier and the product finally
  share one name.
- The `com.muhta.` prefix disappears once a domain exists, removing a personal username from the
  product's identity.
- The migration machinery (marker file, lazy credential move, cleanup journal)
  already exists in the prototype for other purposes and is being reused rather
  than invented.

**Costs**

- A release where a bug in the migration loses someone's settings is the worst
  possible bug in a tool whose entire value is remembering configuration. The
  tests for this need to be disproportionate to the size of the change.
- Dual-emitting events for one release is ugly and has to be remembered and
  removed.
- Users who look in Credential Manager or `%APPDATA%` will see both old and new
  entries for a while, which looks like a bug and needs a documented explanation.
- Windows Credential Manager operations can fail for reasons outside the
  application's control, so the lazy migration must tolerate a permanently failing
  delete without losing the credential.

**Neutral**

- The old directory and old credential entries are left behind rather than
  cleaned up aggressively. Leaving litter is preferable to deleting the only copy
  of something.

## Alternatives considered

**Leave the identifiers as they are, forever.** The status quo, and defensible —
they are invisible to most users. Rejected: the confusion compounds as the project
becomes public and as a second and third component adopt a namespace.

**Rename without migration and tell the user to reconfigure.** Rejected: it
silently discards credentials and settings, which is exactly the behaviour the
product promises never to have.

**Support both namespaces indefinitely with no migration.** Rejected: two code
paths for every persisted document, forever, and a growing chance that one of them
rots untested.

**Do the rename during the platform build-out.** Rejected: mixing a data migration
with an architectural change makes a failure impossible to attribute.
