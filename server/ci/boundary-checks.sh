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
section "Container-runtime sockets: one probe, in Agent, that sends nothing"

# M1C's RuntimeProbe (plan section 3.4) needs the candidate paths, as a
# compiled-in constant in Agent. That one file is the only place in the
# workspace or the service assets allowed to name a runtime socket. Core, the
# shared crates, atriumctl and the units must not, so no code path outside the
# probe can even spell one (ADR-013, criteria 35, 36).
PROBE_FILE="${CRATES}/atrium-agent/src/probe.rs"
runtime_socket_hits="$(grep -rnE 'docker\.sock|podman\.sock|/var/run/docker|/run/docker|/run/podman' \
    "${CRATES}" "${PACKAGING}" 2>/dev/null | grep -v "^${PROBE_FILE}:" || true)"
if [ -n "${runtime_socket_hits}" ]; then
    fail "reference to a container-runtime socket outside Agent's probe (ADR-013):"
    printf '%s\n' "${runtime_socket_hits}" >&2
else
    pass "no container-runtime socket referenced outside atrium-agent/src/probe.rs"
fi

# The probe connects and closes. It must never write to, read from or speak
# the API of a runtime: no write or send call, no read, no HTTP.
# Production code only: the module's own tests read from a fake runtime to
# prove the probe sent nothing.
probe_code="$(sed '/#\[cfg(test)\]/,$d' "${PROBE_FILE}")"
probe_io_hits="$(printf '%s\n' "${probe_code}" |
    grep -nE '\.write|write_all|\.send|\.read|AsyncWrite|AsyncRead|http|GET /' |
    grep -vE '^[0-9]+:\s*//' || true)"
if [ -n "${probe_io_hits}" ]; then
    fail "the runtime probe reads, writes or speaks to a runtime:"
    printf '%s\n' "${probe_io_hits}" >&2
else
    pass "the runtime probe sends and reads nothing (connect, close)"
fi

# ---------------------------------------------------------------------------
section "The Agent operation table (criteria 38, 46)"

# Plan section 18.3, gate 3. The enum is extracted from source and checked
# for parameter types that could carry a path, a command, bytes, a key or a
# trust flag. M1's table is two unit variants, so any parameter at all is a
# change that must come with a review and an update to this gate.
MESSAGES="${CRATES}/atrium-protocol/src/messages.rs"
agent_op="$(awk '/^pub enum AgentOp \{/,/^\}/' "${MESSAGES}" | grep -vE '^\s*///')"
if [ -z "${agent_op}" ]; then
    fail "could not find 'pub enum AgentOp' in ${MESSAGES}"
elif printf '%s\n' "${agent_op}" |
    grep -qE 'String|PathBuf|Path|OsString|str|Vec<|\[u8|Box<|Value|Key|Cert|Trust|Token|Secret|Command|Argv|Unit|Package|Container|bool'; then
    fail "AgentOp carries a forbidden parameter type:"
    printf '%s\n' "${agent_op}" >&2
elif printf '%s\n' "${agent_op}" | sed '1d;$d' | grep -qE '[({]'; then
    fail "AgentOp has a parameterised variant; M1 has none (plan section 3.4):"
    printf '%s\n' "${agent_op}" >&2
else
    pass "AgentOp is $(printf '%s\n' "${agent_op}" | sed '1d;$d' | grep -cE '^\s*[A-Z][A-Za-z]*,') parameterless variants"
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
