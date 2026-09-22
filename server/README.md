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
cargo fmt --manifest-path server/Cargo.toml --check
cargo clippy --manifest-path server/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path server/Cargo.toml
bash server/ci/boundary-checks.sh
```

## State of the implementation

M1A only: the workspace, the crate skeletons, the process lifecycle and the
gates. The binaries start, log, signal readiness and shut down cleanly, and do
nothing else — no HTTP, no TLS, no database, no identity, no protocol, no
discovery. Each of those arrives in its own pass; the order and the acceptance
criteria are in [`../docs/M1-IMPLEMENTATION-PLAN.md`](../docs/M1-IMPLEMENTATION-PLAN.md).
