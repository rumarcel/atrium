# Atrium server workspace

Core, Agent, the console tool and the crates they share. This is a **second,
independent Cargo workspace**: the Windows desktop application in `../src-tauri`
keeps its own manifest, its own `Cargo.lock` and its own CI job, and neither
workspace is a member of the other.

Targets Linux. Debian 12/13 and Ubuntu Server 24.04/26.04 are the supported
server platforms; see [`../docs/PLATFORM-MATRIX.md`](../docs/PLATFORM-MATRIX.md).

## Crates

| Crate | What it is | Rule that matters |
| --- | --- | --- |
| `atrium-protocol` | Core-to-Agent constants and, from M1C, the typed operation table | pure: no I/O, no paths, no runtime |
| `atrium-pairing` | the pairing secret codec, transcript, proofs, device token, wire types and client state machine (M1E) | pure: no I/O, no randomness source |
| `atrium-api-types` | public API constants; from M1D the DTOs and error codes | pure: no I/O |
| `atrium-core` | the unprivileged server component | never root, no container runtime, never depends on `atrium-agent` |
| `atrium-agent` | the privileged component | root, one Unix socket, no HTTP/TLS/SQL crate |
| `atrium-client` | the native API client | platform-neutral; must keep building on Windows |
| `atriumctl` | the console tool | owns what must not be reachable over the network |

Crate boundaries here are security boundaries. They are enforced mechanically,
not by review — see `ci/boundary-checks.sh`.

## Checks

```sh
cargo fmt --manifest-path server/Cargo.toml --all --check
cargo clippy --manifest-path server/Cargo.toml --all-targets -- -D warnings
cargo build --manifest-path server/Cargo.toml --all-targets
cargo test --manifest-path server/Cargo.toml
bash server/ci/boundary-checks.sh
```

The privileged suites (`docs/M1-TEST-PLAN.md` section 5) need root, an
`atrium` system user and `/usr/bin/systemd-socket-activate`, and run their
already-built test binaries under `sudo`:

```sh
cargo test --manifest-path server/Cargo.toml -p atriumctl --test privileged --no-run
cargo test --manifest-path server/Cargo.toml -p atrium-agent --test privilege_boundary --no-run
sudo <each path printed above> --ignored --test-threads=1
```

## State of the implementation

**M1A** — the workspace, the crate skeletons, the process lifecycle and the
gates.

**M1B** — identity and state:

- `atrium-core init-identity` creates, once, at installation and as the
  `atrium` user: `server_id`, the ECDSA P-256 identity key, `secrets.key`, the
  state database at schema 1 and the first certificate. It refuses a partial
  or inconsistent tree and changes nothing on a finished one (exit 3).
- `atrium-core` verifies the identity (ownership and modes included), opens the
  database (integrity check, backup-before-migrate), keeps the certificate
  current from the same key, and otherwise runs in **recovery mode**: up,
  reporting, changing nothing.
- `atriumctl pair`, `diagnostics`, `restore --list`, `restore --from <name>`
  and `rotate-identity` — console only; each drops to `atrium` for good before
  it touches state.

**M1C** — the Core-to-Agent protocol. `atrium-agent` serves two parameterless,
read-only operations, `AgentInfo` and a passive `RuntimeProbe` (socket
presence by metadata, never a connection), on
`/run/atrium/agent.sock`, to Core's uid only (`SO_PEERCRED`). It journals
every connection in its root-only state directory. `atrium-core` calls it in
normal mode, audits every call as `agent.call`, and derives the `privileged`
and `container` capabilities, or reports `agent_unreachable` and tries nothing
else.

**M1D** — the HTTPS listener. TLS with the identity key (1.3, and 1.2 with
extended master secret; no resumption; HTTP/1.1 only), a literal route table
with per-mode policies (`/healthz`, a placeholder `/`, and in recovery the
redacted diagnostics), a `Host` allowlist taken from the certificate's own
names, deny-by-default browser policy with no CORS headers, RFC 9457 errors
from a closed set, and a per-connection context holding the TLS exporter for
M1E's pairing.

**M1E** — pairing and devices. `atriumctl pair` arms a 128-bit, 15-minute,
single-use secret for the native TLS-exporter profile, sealed under
`secrets.key` ([ADR-019](../docs/adr/0019-sealed-pairing-secret.md)), and
shows it once. `pair/info`, `pair/begin` and `pair/complete` (TLS 1.3 only)
bind both proofs to the connection's exporter and the server's SPKI; success
creates an `owner` device in the transaction that consumes the secret and
returns a 256-bit token, of which Core keeps only `SHA-256`. Device routes
(`/api/v1/me`, `/api/v1/devices`, `DELETE /api/v1/devices/{deviceId}`)
authenticate at one point in dispatch, with no cache. Per-source limits: 8
connections, and a token bucket on the pairing routes. `rotate-identity` now
arms a fresh code.

**M1F** — the system domain, native. `/api/v1/system`, `/system/metrics`,
`/system/capabilities`, `/system/diagnostics`, `/network/interfaces` and
`/storage/filesystems`, device-only, read from `/proc`, `/sys`,
`/etc/os-release`, `getifaddrs` and `statvfs` — no Glances, Homarr,
Cockpit, runtime API or monitoring library. A value that cannot be read is
`null` with a closed reason, never `0`; CPU usage comes from a background
sampler and is `first_sample_pending` until it has two samples. Before
M1F, [ADR-020](../docs/adr/0020-no-failed-proof-lock.md) removed the
five-failure pairing lock.

Still absent: discovery (M1G). The order and the acceptance
criteria are in [`../docs/M1-IMPLEMENTATION-PLAN.md`](../docs/M1-IMPLEMENTATION-PLAN.md).

`ATRIUM_ROOT=<dir>` relocates the whole file tree (`<dir>/etc/atrium`,
`<dir>/var/lib/atrium`) for the test suites. It is not a deployment setting.
