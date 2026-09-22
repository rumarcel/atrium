#!/usr/bin/env bash
#
# Architectural gates for the Atrium server workspace.
#
# These exist so that the boundaries in docs/ARCHITECTURE.md and the ADRs are
# enforced by the build rather than by a reviewer remembering them. They run in
# CI on every change and can be run by hand from anywhere:
#
#     bash server/ci/boundary-checks.sh
#
# Dependency rules are checked against cargo's own metadata, not against the
# text of a manifest, so a forbidden crate cannot arrive transitively unnoticed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "${SCRIPT_DIR}/.." && pwd)"
MANIFEST="${WORKSPACE}/Cargo.toml"
CRATES="${WORKSPACE}/crates"
PACKAGING="${WORKSPACE}/packaging"

failures=0

fail() {
    printf '  FAIL  %s\n' "$1" >&2
    failures=$((failures + 1))
}

pass() {
    printf '  ok    %s\n' "$1"
}

section() {
    printf '\n== %s\n' "$1"
}

# Normal (non-dev, non-build) dependency closure of one package, one name per
# line. This is cargo's resolved graph, so it covers transitive arrivals.
deps_of() {
    cargo tree \
        --manifest-path "${MANIFEST}" \
        --package "$1" \
        --edges normal \
        --prefix none \
        --no-dedupe \
        --locked 2>/dev/null |
        awk 'NF {print $1}' |
        sort -u
}

# Fails if any dependency of $1 matches the extended regex $2.
forbid_dependency() {
    local package="$1" pattern="$2" reason="$3" hits
    hits="$(deps_of "${package}" | grep -Ex "${pattern}" || true)"
    if [ -n "${hits}" ]; then
        fail "${package} depends on $(echo "${hits}" | tr '\n' ' ')— ${reason}"
    else
        pass "${package}: no ${reason}"
    fi
}

# ---------------------------------------------------------------------------
section "Dependency direction and forbidden dependencies (cargo metadata)"

CONTAINER_RUNTIME='bollard|docker-api|dockworker|shiplift|podman-api|podman-rest-client|containerd-client|cri-.*'
HTTP_CRATES='axum|hyper|hyper-util|reqwest|actix-web|warp|tiny_http|ureq'
TLS_CRATES='rustls|tokio-rustls|native-tls|openssl|openssl-sys|boring|schannel'
SQL_CRATES='rusqlite|libsqlite3-sys|sqlx|diesel|sea-orm'
IO_CRATES='tokio|mio|socket2|nix|rustix|libc|async-std|smol'

# ADR-001, ADR-013: Core is unprivileged and has no runtime access of any kind.
forbid_dependency atrium-core 'atrium-agent' "dependency on the privileged component (ADR-001)"
forbid_dependency atrium-core "${CONTAINER_RUNTIME}" "container-runtime client (ADR-013, criterion 36)"

# ADR-004: Agent speaks one typed protocol on one Unix socket.
forbid_dependency atrium-agent "${HTTP_CRATES}" "HTTP crate"
forbid_dependency atrium-agent "${TLS_CRATES}" "TLS crate"
forbid_dependency atrium-agent "${SQL_CRATES}" "SQL crate"
forbid_dependency atrium-agent "${CONTAINER_RUNTIME}" "container-runtime client"

# The shared crates are types and codecs. No I/O, so the Core-to-Agent contract
# stays reviewable by reading one small crate.
for pure in atrium-protocol atrium-pairing atrium-api-types; do
    forbid_dependency "${pure}" "${IO_CRATES}|${HTTP_CRATES}|${TLS_CRATES}|${SQL_CRATES}" \
        "I/O dependency in a pure crate"
done

# The client ships inside the Windows desktop application. Nothing Linux-only
# or server-only may reach it.
forbid_dependency atrium-client "atrium-core|atrium-agent|${SQL_CRATES}|axum|nix|rustix" \
    "server-only or Linux-only dependency in the desktop client crate"

# ---------------------------------------------------------------------------
section "No shell or process execution in production code"

# Scoped to src/ of the two server binaries: integration tests under tests/ may
# legitimately spawn the binary they are testing, and documentation may mention
# these strings. Production code may not.
shell_hits="$(grep -rnE 'Command::new|process::Command|\bsh -c\b|\bbash -c\b' \
    "${CRATES}/atrium-core/src" "${CRATES}/atrium-agent/src" "${CRATES}/atriumctl/src" \
    2>/dev/null || true)"
if [ -n "${shell_hits}" ]; then
    fail "process execution in production code (criterion 47):"
    printf '%s\n' "${shell_hits}" >&2
else
    pass "no process execution in atrium-core, atrium-agent or atriumctl sources"
fi

# ---------------------------------------------------------------------------
section "No container-runtime socket anywhere"

runtime_socket_hits="$(grep -rnE 'docker\.sock|podman\.sock|/var/run/docker|/run/docker' \
    "${CRATES}" "${PACKAGING}" 2>/dev/null || true)"
if [ -n "${runtime_socket_hits}" ]; then
    fail "reference to a container-runtime socket (ADR-013):"
    printf '%s\n' "${runtime_socket_hits}" >&2
else
    pass "no container-runtime socket referenced in code or service assets"
fi

# ---------------------------------------------------------------------------
section "Unsafe code is confined to one audited site"

unsafe_files="$(grep -rlE '(^|[^_[:alnum:]])unsafe[[:space:]]*\{' "${CRATES}" --include='*.rs' \
    2>/dev/null | sed "s|${CRATES}/||" | sort || true)"
expected_unsafe="atrium-agent/src/lib.rs"
if [ "${unsafe_files}" != "${expected_unsafe}" ]; then
    fail "unsafe blocks outside the audited site. expected only '${expected_unsafe}', found: ${unsafe_files:-none}"
else
    pass "unsafe confined to ${expected_unsafe} (socket-activation descriptor adoption)"
fi

# ---------------------------------------------------------------------------
section "Service assets"

if find "${PACKAGING}" -iname '*sudoers*' | grep -q .; then
    fail "a sudoers file exists in packaging; ADR-004 removed sudo entirely"
else
    pass "no sudoers file"
fi

# ADR-015 reserves the prototype's identifiers. The canary must not touch them.
prototype_hits="$(grep -rnE 'personal-hub|personalhub|:9473|61208' "${PACKAGING}" 2>/dev/null || true)"
if [ -n "${prototype_hits}" ]; then
    fail "service asset references a reserved prototype identifier (ADR-015):"
    printf '%s\n' "${prototype_hits}" >&2
else
    pass "no reserved prototype identifier in service assets"
fi

# ---------------------------------------------------------------------------
section "Result"

if [ "${failures}" -ne 0 ]; then
    printf '\n%d boundary check(s) failed.\n' "${failures}" >&2
    exit 1
fi
printf '\nAll boundary checks passed.\n'
