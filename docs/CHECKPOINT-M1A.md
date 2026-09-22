# Checkpoint — after M1A

**Status: M1A COMPLETE · M1B NOT STARTED · PROJECT INTENTIONALLY PAUSED**

| | |
| --- | --- |
| Checkpoint date | 2026-09-22 |
| Checkpoint commit | `9b9d18f` — `feat(server): M1A workspace, process skeletons and architectural gates` |
| Branch | `product/m1-alpha` |
| Tag | `m1a-checkpoint` |
| Next pass | **M1B — identity, certificates, persistent-state/recovery skeleton** |

The pause is deliberate: the product owner is working on other things. Nothing
here is blocked, half-finished or waiting on a decision.

## Where the architecture stands

Atrium is becoming a server platform: an unprivileged **Core**, a privileged
**Agent** that owns the container runtime, and clients that are views onto one
API. The Windows desktop application in `src-tauri/` is the **prototype** — it
still builds, still works, and is untouched by any of this.

Sixteen ADRs are accepted and frozen. The whole target design lives under
[`docs/`](README.md); the pass-by-pass plan is
[`M1-IMPLEMENTATION-PLAN.md`](M1-IMPLEMENTATION-PLAN.md) §17, and the 48 binding
acceptance criteria are in [`ROADMAP.md`](ROADMAP.md#m1--acceptance-criteria).

## What M1A delivered

- `server/` — a **second, independent** Cargo workspace with its own lockfile.
  The repository root is deliberately **not** a workspace, and `src-tauri/` was
  not restructured.
- Seven crates: `atrium-protocol`, `atrium-pairing`, `atrium-api-types`
  (all pure, zero dependencies), `atrium-core`, `atrium-agent`, `atrium-client`
  (platform-neutral), `atriumctl`.
- Lifecycle skeletons for Core and Agent: deterministic startup, JSON logs with
  process identity and version, `sd_notify` readiness, clean `SIGTERM`, non-zero
  exit on startup failure. Core refuses to run as root; Agent refuses a socket
  path from the environment while running as root.
- Three systemd units in the Atrium namespace.
- `server/ci/boundary-checks.sh` — dependency-direction and forbidden-dependency
  gates against cargo metadata, no-shell-execution, no runtime socket, unsafe
  confinement, no sudoers, no reserved prototype identifier. Each gate was
  verified to fire by introducing a violation.
- A fourth CI job for the server workspace, alongside the untouched three.

**M1A deliberately contains no** HTTP server, TLS, identity, database, protocol,
discovery, container access or invented placeholder data.

## What is known to pass, and where

Verified locally on the Windows development workstation:

- `cargo fmt --all --check`
- `cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings`
- `cargo check --target x86_64-unknown-linux-gnu --all-targets`
- `cargo test` for the five portable crates — **24 tests**
- `bash server/ci/boundary-checks.sh` — all gates

**Relies on Linux CI** — 30 further tests that cannot execute on Windows: the
Agent listener and socket tests, the Core root guard, `notify`/`signals`, and
both process-lifecycle suites (spawn, readiness, `SIGTERM`, exit codes, no TCP
socket held). Plus the CI-only `sudo` check that Core refuses uid 0.

## Known limitations

- The Linux binaries are **cross-checked, not executed** on the development
  workstation (no WSL, no Linux toolchain). CI is the only place they run.
- `systemd-analyze verify` runs in CI as **informational** (`continue-on-error`),
  because its exit status depends on runner state. The blocking check on unit
  content is the deployment-policy test.
- `cargo deny` is configured (`server/deny.toml`) but not yet wired into CI.
- Criteria **5** and **41** remain **partial** by design; **6** is specification
  only until M1B creates the files.

## Prototype coexistence

Untouched and operational. `src-tauri/`, `src/`, `tests/`, `server-agent/` and
`public/` have zero changes from the platform work. ADR-015's reserved
identifiers — ports, units, users, paths — are enforced by a gate, so the canary
cannot collide with the prototype's agent.

## Reading list before continuing

In this order, and before touching code:

1. This file.
2. [`M1-IMPLEMENTATION-PLAN.md`](M1-IMPLEMENTATION-PLAN.md) — §4 persistence,
   §5 identity lifecycle, §17 (the **M1B** entry and the M1A "As built" note),
   §19 migration and rollback, §20 the acceptance matrix.
3. [`M1-TEST-PLAN.md`](M1-TEST-PLAN.md) — §4.5 recovery, §4.6 identity, §5
   privileged tests.
4. [`ROADMAP.md`](ROADMAP.md#m1--acceptance-criteria) — criteria 1–48 and
   *Criteria amended during planning*.
5. [`SECURITY.md`](SECURITY.md) §9 and §18–19.
6. ADRs **003** (pairing, binding profiles, SHA-256 token verifier), **007**
   (language and packaging), **012** (state store and migrations), **015**
   (canary coexistence), **016** (trust roots).
7. `server/README.md` and `server/ci/boundary-checks.sh`.

## Warnings for whoever resumes

- **Do not skip to M1C or later.** The passes are ordered because each one's
  tests depend on the previous one's structure. M1B is next.
- **Do not redesign an accepted ADR** without first finding a concrete
  contradiction. If you find one, report it and stop — that is the documented
  process, and it has already caught four real defects.
- **Do not upgrade dependencies automatically on resume.** The lockfile is
  committed on purpose. Check advisories first, then decide deliberately.
- **Do not weaken an acceptance criterion** to make a pass finish. Two have been
  amended, both in the direction of a stronger property, both recorded.

## First actions on resume

1. `git fetch` / `git pull`.
2. Read this checkpoint.
3. Run the current checks: the five commands listed above, and the CI run for
   the tip commit.
4. Check dependency and security advisories (`cargo audit` / `cargo deny check`)
   before changing any version.
5. Only then begin M1B.

## M1B handoff notes

Carried forward from M1A, so they are not rediscovered:

- **`init-identity` runs before `/etc/atrium` is locked down.** The installer
  creates the directory writable, runs `atrium-core init-identity` as the
  `atrium` user, then changes ownership and mode. The long-running Core never
  has permission to create identity material.
- **Final ownership is `/etc/atrium` = `root:atrium 0750`.** Group traverse and
  read, **no group write**. A `root:root` directory would deny the read Core
  depends on — this was corrected before M1A and must not regress.
- **Sensitive readable files are `root:atrium 0640`**: `tls.key`,
  `identity.json`, `secrets.key`.
- **The invariant:** the running Core must be able to **read** the identity key
  and must not be able to **replace** it — not by write, truncate, unlink,
  rename or chmod.
- **M1B owns the real OS-level write-denial test.** M1A could only specify the
  expected metadata; proving it needs the files to exist.
- **Privileged tests need a Linux user/root fixture.** They cannot run on the
  Windows workstation and need the `server-privileged` CI job sketched in the
  test plan, with the `atrium` user created and the directories in place.
