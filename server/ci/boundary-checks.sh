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
# The pairing construction takes every random value as an argument, so its
# vectors are fixed and no code path in it can pick a weak source.
forbid_dependency atrium-pairing 'getrandom|rand|rand_core|rand_chacha' \
    "randomness source in the pure pairing crate"

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
section "Container-runtime sockets: one passive probe, in Agent"

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

# The probe is passive (M1D, §1A): it inspects metadata and never connects,
# because a connection wakes a socket-activated runtime. No connect, no
# stream, no write or send, no read, no HTTP.
# Production code only: the module's own tests read from a fake runtime to
# prove the probe sent nothing.
probe_code="$(sed '/#\[cfg(test)\]/,$d' "${PROBE_FILE}")"
probe_io_hits="$(printf '%s\n' "${probe_code}" |
    grep -nE 'connect|UnixStream|TcpStream|\.write|write_all|\.send|\.read|AsyncWrite|AsyncRead|http|GET /' |
    grep -vE '^[0-9]+:\s*//' || true)"
if [ -n "${probe_io_hits}" ]; then
    fail "the runtime probe connects to, reads from or writes to a runtime:"
    printf '%s\n' "${probe_io_hits}" >&2
else
    pass "the runtime probe is passive: no connect, no read, no write"
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
section "The network boundary cannot reach privilege or state (M1D, M1E)"

# Core's HTTP module holds no database handle of its own. Its production code
# must not name the Agent client, the state database, identity rotation,
# restore, init, a raw file write or process execution: unauthenticated
# network input must have no path to any of them. From M1E it reaches state
# only through the pairing and device services (crate::pairing,
# crate::devices), which are gated below. The module's test files are
# excluded; everything else under src/http is checked.
HTTP_DIR="${CRATES}/atrium-core/src/http"
HTTP_TESTS="^${HTTP_DIR}/(tests|pairing_tests)\.rs:"
http_hits="$(grep -rnE 'agentclient|agentmonitor|AgentClient|crate::db|rusqlite|crate::rotate|restore::|crate::init|fs::write|OpenOptions|Command::' \
    "${HTTP_DIR}" --include='*.rs' 2>/dev/null | grep -vE "${HTTP_TESTS}" || true)"
if [ -n "${http_hits}" ]; then
    fail "the HTTP module reaches something network input must never reach:"
    printf '%s\n' "${http_hits}" >&2
else
    pass "src/http names no Agent client, database, rotation, restore, file write or process"
fi

# The services the network reaches: pairing and devices. They own the state
# they change and nothing else — no Agent, no process, no identity files, no
# rotation or restore. Pairing never contacts Agent.
SERVICES=("${CRATES}/atrium-core/src/pairing" "${CRATES}/atrium-core/src/devices.rs")
service_hits="$(grep -rnE 'agentclient|agentmonitor|AgentClient|crate::rotate|restore::|crate::init|fs::write|OpenOptions|Command::|atrium_protocol' \
    "${SERVICES[@]}" --include='*.rs' 2>/dev/null | grep -v '/src/pairing/tests\.rs:' || true)"
if [ -n "${service_hits}" ]; then
    fail "a network-reachable service reaches Agent, a process or the identity files:"
    printf '%s\n' "${service_hits}" >&2
else
    pass "pairing and devices name no Agent, process, rotation, restore or file write"
fi

# There is one listener and it is TLS: no plaintext bind anywhere else in
# Core's production code.
bind_hits="$(grep -rnE 'TcpListener::bind' "${CRATES}/atrium-core/src" --include='*.rs' 2>/dev/null |
    grep -vE '^[^:]*/src/lib\.rs:|/src/http/(tests|pairing_tests)\.rs:' || true)"
if [ -n "${bind_hits}" ]; then
    fail "a TCP listener is bound outside Core's one TLS listener setup:"
    printf '%s\n' "${bind_hits}" >&2
else
    pass "Core binds TCP only where the TLS listener is set up"
fi

# ---------------------------------------------------------------------------
section "Pairing and device secrets (M1E)"

PAIRING_SRC="${CRATES}/atrium-pairing/src"
CORE_SRC="${CRATES}/atrium-core/src"
PRODUCTION_CORE=("${CORE_SRC}/pairing/mod.rs" "${CORE_SRC}/pairing/seal.rs" \
    "${CORE_SRC}/devices.rs" "${CORE_SRC}/db/pairing.rs" "${CORE_SRC}/http/dispatch.rs" \
    "${CORE_SRC}/http/mod.rs" "${CORE_SRC}/http/limits.rs" "${CORE_SRC}/lib.rs" \
    "${CRATES}/atriumctl/src/cmd/pair.rs")

# Test plan 2.1 and 4.2: the secret, the token and the verifier are compared
# only in constant time. None of their types may derive or implement
# equality, so `==` on them does not compile.
eq_hits="$(grep -nE -B3 'pub struct (PairingSecret|DeviceToken|TokenDigest)\b' \
    "${PAIRING_SRC}/secret.rs" "${PAIRING_SRC}/token.rs" | grep -E 'PartialEq|Eq\b' || true)"
impl_eq="$(grep -nE 'impl (Partial)?Eq for (PairingSecret|DeviceToken|TokenDigest)' -r "${PAIRING_SRC}" || true)"
if [ -n "${eq_hits}${impl_eq}" ]; then
    fail "a secret type has equality; compare with ct_eq only:"
    printf '%s\n' "${eq_hits}${impl_eq}" >&2
else
    pass "PairingSecret, DeviceToken and TokenDigest have no ==, only constant-time ct_eq"
fi

# Plan 7.3: no verification cache sits between a request and the digest
# lookup, so a revoked token fails on its next request.
cache_hits="$(grep -nE 'HashMap|BTreeMap|LruCache|lru::|moka|cached|OnceCell|OnceLock|LazyLock|thread_local' \
    "${CORE_SRC}/devices.rs" "${CORE_SRC}/db/pairing.rs" || true)"
if [ -n "${cache_hits}" ]; then
    fail "device authentication holds state that could outlive a revocation:"
    printf '%s\n' "${cache_hits}" >&2
else
    pass "no authentication cache in devices or its queries"
fi

# Test plan 4.2a: the accepted profile comes from pairing_state, never from a
# request. Arming names only the native constant, and the request's profile
# field is read in exactly one place, to be compared against the armed one.
armed_profiles="$(grep -nE 'profile: *[A-Za-z_:.]+' "${CORE_SRC}/pairing/mod.rs" | grep -vE 'NATIVE_PROFILE|&armed\.profile|&self\.|profile: &' || true)"
request_profile="$(grep -rn 'binding_profile' "${CORE_SRC}" --include='*.rs' | grep -vE '/(tests|pairing_tests)\.rs:' || true)"
if [ -n "${armed_profiles}" ] || [ "$(printf '%s\n' "${request_profile}" | grep -c .)" -ne 1 ]; then
    fail "the armed profile could come from somewhere other than the native constant:"
    printf '%s\n%s\n' "${armed_profiles}" "${request_profile}" >&2
else
    pass "only the native profile is armed; the request's profile is only compared"
fi

# Limits and sources come from the socket. No client-controlled forwarding
# header is read anywhere in Core's production code.
forward_hits="$(grep -rniE 'x-forwarded|forwarded-for|"forwarded"|x-real-ip' "${CORE_SRC}" --include='*.rs' |
    grep -vE '/(tests|pairing_tests)\.rs:' | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "${forward_hits}" ]; then
    fail "Core reads a forwarding header:"
    printf '%s\n' "${forward_hits}" >&2
else
    pass "no forwarding header is read; the source is the accepted socket"
fi

# Nothing secret is logged: no tracing field or format argument names the
# secret, the token, a proof, a digest or the exporter.
log_hits="$(grep -nE '[%?][a-z_.]*(secret|token|proof|exporter|digest|nonce)|\b[a-z_]*(secret|token|proof|exporter|digest|nonce)[a-z_]*[[:space:]]*=[[:space:]]*[%?][a-z_&(]|^[[:space:]]*(secret|token|proof_c|proof_s|exporter|digest|token_digest)[[:space:]]*=[[:space:]]*[a-z]' \
    "${PRODUCTION_CORE[@]}" || true)"
if [ -n "${log_hits}" ]; then
    fail "a log statement may carry secret material:"
    printf '%s\n' "${log_hits}" >&2
else
    pass "no log field carries a secret, token, proof, digest or exporter"
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
