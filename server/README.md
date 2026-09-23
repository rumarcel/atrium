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
| `atrium-pairing` | pairing identifiers and sizes; from M1E the codec and proofs | pure: no I/O |
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

The privileged suite (`docs/M1-TEST-PLAN.md` section 5) needs root and an
`atrium` system user, and runs its already-built test binary under `sudo`:

```sh
cargo test --manifest-path server/Cargo.toml -p atriumctl --test privileged --no-run
sudo <path printed above> --ignored --test-threads=1
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
- `atriumctl diagnostics`, `restore --list`, `restore --from <name>` and
  `rotate-identity` — console only; each drops to `atrium` for good before it
  touches state.

Still absent, each in its own pass: HTTP and TLS serving, the Core-to-Agent
protocol, pairing, discovery and system providers. The order and the acceptance
criteria are in [`../docs/M1-IMPLEMENTATION-PLAN.md`](../docs/M1-IMPLEMENTATION-PLAN.md).

`ATRIUM_ROOT=<dir>` relocates the whole file tree (`<dir>/etc/atrium`,
`<dir>/var/lib/atrium`) for the test suites. It is not a deployment setting.
