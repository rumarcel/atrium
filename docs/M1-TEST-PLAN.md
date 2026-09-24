# M1 — test plan

Status: plan only. Companion to
[`M1-IMPLEMENTATION-PLAN.md`](M1-IMPLEMENTATION-PLAN.md). Criterion numbers refer
to [`ROADMAP.md`](ROADMAP.md#m1--acceptance-criteria) and are binding.

## 0. The rule that shapes this document

**A criterion about operating-system behaviour is proved by operating-system
behaviour.** Where a criterion says "runs as `atrium`", "returns `EACCES`",
"survives a reboot" or "the socket is `0660 root:atrium`", a unit test that mocks
the syscall is not evidence and is not accepted as one. Unit tests exist here to
make the system tests short and to catch regressions fast — never to stand in for
them.

Conversely, a criterion about a *codec* or a *state machine* is proved by unit and
property tests, because running those on a VM proves less, not more.

Each section below states which layer owns which criterion.

## 1. Test layers

| Layer | Where it runs | Owns |
| --- | --- | --- |
| **L1 Unit** | `cargo test`, any platform | codecs, parsers, state machines, error mapping |
| **L2 Property / fuzz** | `cargo test` with `proptest` | encode/decode round-trips, parser robustness |
| **L3 Integration (in-process)** | `cargo test`, Linux | HTTP surface against a real rustls listener and a real SQLite file in a temp dir |
| **L4 Integration (privileged)** | `cargo test` as root, Linux | peer-credential rejection, file ownership, Agent journal correlation |
| **L5 Systemd** | container or VM with systemd | unit correctness, socket permissions, restart and boot behaviour |
| **L6 VM acceptance** | clean distro VMs | install, reboot, IP change, two NICs, Docker present, no network |
| **L7 Canary** | the owner's real server | coexistence with the running prototype |

---

## 2. Unit tests (L1)

### 2.1 Pairing secret codec — `atrium-pairing`

Criterion 12 is a list of assertions; each becomes a named test.

| Test | Asserts |
| --- | --- |
| `generator_emits_exactly_16_bytes` | the generator's output length is 16, from `getrandom` |
| `encoder_emits_exactly_26_symbols` | length 26, alphabet ⊆ `0123456789ABCDEFGHJKMNPQRSTVWXYZ` |
| `alphabet_excludes_confusables` | `I`, `L`, `O`, `U` never appear in output |
| `hyphens_and_whitespace_are_ignored` | grouped, ungrouped, tab- and newline-separated forms decode identically |
| `uppercasing_is_ascii_only_under_turkish_locale` | with `LC_ALL=tr_TR.UTF-8` set for the process, a lowercase secret containing `i` decodes to the same bytes. **Skipped with a loud message if the locale is not generated** — and generated in CI, so it never silently skips |
| `crockford_confusables_map` | `O→0`, `I→1`, `L→1`, both cases |
| `rejects_25_and_27_symbols` | both rejected, distinct error |
| `rejects_non_canonical_padding` | a 26th symbol with non-zero low bits is rejected; the four encodings of one value collapse to exactly one accepted form |
| `rejects_out_of_alphabet` | `U`, `!`, emoji, NUL rejected |
| `qr_payload_decodes_identically` | canonical 26 symbols, no hyphens, decode equal to the display form |
| `comparison_is_on_decoded_bytes` | a compile-level test: the compare function's signature takes `[u8; 16]`, and a source gate forbids `==` on the secret's string type |

**Negative-space test:** `secret_type_has_no_display_impl` — the plaintext secret
newtype must not implement `Display`, and its `Debug` must emit `<redacted>`.

### 2.2 Transcript and proofs

| Test | Asserts |
| --- | --- |
| `transcript_is_fixed_length` | `T` is exactly 208 bytes (profile hash, serverId, spki, cb, both nonces, device hash); each field's offset is asserted |
| `proof_vectors` | frozen test vectors for `K`, `proofC`, `proofS` from fixed inputs, committed to the repository so a refactor cannot silently change the construction |
| `client_and_server_labels_differ` | `proofC != proofS` for identical inputs |
| `changing_any_field_changes_the_proof` | flip one byte in each of `server_id`, `spki`, `cb`, both nonces, device hash → proof changes (6 sub-cases) |

### 2.3 Provider parsers

Fixture trees under `server/crates/atrium-core/tests/fixtures/`, each a snapshot
of a real `/proc` and `/sys` subtree plus hand-made hostile variants.

| Test | Asserts |
| --- | --- |
| `proc_stat_delta` | usage computed from two samples; single sample → `null` + `first_sample_pending` |
| `meminfo_without_memavailable` | `null` + `mem_available_unsupported`, **not** a computed guess |
| `meminfo_zero_swap` | `swap_absent`, not `0` |
| `loadavg_malformed` | `null` + `procfs_unreadable`, no panic |
| `mountinfo_bind_collapse` | duplicate `major:minor` collapses; `alsoMountedAt` populated |
| `mountinfo_pseudo_excluded` | every fstype in the denylist absent from the result |
| `statvfs_denied` | mount still listed, `usage: null`, `metadata_unavailable` |
| `hwmon_absent` | `temperatures: []` **and** capability `missing: temperature/no_hwmon_sensors` |
| `hwmon_label_missing` | sensor reported with `label: null`, not a fabricated name |
| `sysfs_speed_minus_one` | `speedMbps: null` + `not_reported_by_driver` |
| `os_release_missing` | `null` + `os_release_unreadable` |
| **`empty_fixture_tree_invents_nothing`** | with every source absent, **no numeric field is `0`** — a recursive walk of the serialized response asserts every leaf is `null` or a reason string |

That last test is the one that keeps the product honest, and it runs on every
commit.

### 2.4 Error model

| Test | Asserts |
| --- | --- |
| `error_codes_are_unique_and_stable` | the enum → code map is injective; codes compared against a committed literal list |
| `error_variants_are_exhaustive` | a `match` over `AtriumError` with no wildcard arm — a new variant breaks the build until it is given a code |
| `problem_json_shape` | `type`, `title`, `status`, `code`, `diagnosis`, `requestId` present on every variant |
| `details_never_contains_secret_types` | `Redacted<T>` serializes to `"<redacted>"` in a problem object |

### 2.5 Protocol codec

`frame_length_prefix_round_trip`, `frame_over_64kib_rejected_before_alloc`,
`unknown_field_rejected`, `truncated_frame`, `non_utf8_payload`,
`version_mismatch_is_terminal`.

---

## 3. Property and fuzz tests (L2)

| Target | Property |
| --- | --- |
| Secret codec | `decode(encode(b)) == b` for all 16-byte inputs; `decode` never panics on arbitrary `String` input; every accepted string re-encodes to its canonical form |
| Protocol frames | `parse` never panics on arbitrary bytes; never allocates more than the declared length |
| `mountinfo` parser | never panics on arbitrary bytes; output size bounded by input line count |
| `/proc/stat` parser | never panics; never divides by zero when two samples are identical |

`proptest` for all four. A `cargo-fuzz` target for the frame parser and the
secret decoder is worthwhile but is **not** an M1 gate — it is set up in M1I and
run out of band.

---

## 4. Integration tests (L3, unprivileged Linux)

Each spins a real Core in-process: real rustls listener on `127.0.0.1:0`, real
SQLite in a `tempfile::TempDir`, a fake clock, and a harness client built on
`atrium-client`.

### 4.1 HTTP surface

| Test | Criterion |
| --- | --- |
| `route_table_matches_expected_literal_list` | 37 |
| `every_response_carries_version_headers` — success **and** every error variant | 27 |
| `unauthenticated_routes_return_401_problem` — sweeps every non-pairing route | 26 |
| `error_response_leaks_nothing` — asserts the 401 body contains no hostname, path, version or device id | 26 |
| `unknown_body_field_rejected` — one case per POST DTO | 28 |
| `body_over_limit_rejected_before_parse` | 28 |
| `host_header_allowlist` — a rebinding-style `Host` is refused before routing | — |
| `origin_header_on_token_request_refused` | — |

### 4.2 Pairing flow

| Test | Criterion |
| --- | --- |
| `happy_path_pairs_and_returns_token` | 11 |
| `proof_s_is_verifiable_by_the_client` | 11, 16 |
| `wrong_secret_returns_generic_rejection` — body byte-identical to the expired case | 13 |
| `five_failures_lock_pairing` — sixth attempt refused even with the correct secret; `atriumctl pair` clears it | 13 |
| `used_secret_cannot_be_reused` | 14 |
| `expired_secret_rejected` — fake clock +15 min | 14 |
| `begin_complete_window_expires` — fake clock +2 min | 14 |
| `complete_on_a_different_connection_is_refused` | 15 (supporting) |
| `first_pairing_claims_the_server` | 17 |
| `second_pairing_without_a_fresh_secret_refused` | 17 |
| `revocation_takes_effect_on_the_next_request` — two devices, one revokes the other, the very next request fails | 20 |
| `revoked_and_unknown_tokens_are_indistinguishable` — identical status, body and timing bucket | 20 |
| `no_authentication_cache_exists` — a source-level assertion that no memoisation sits between the request and the digest lookup, plus a behavioural one: revoke, then issue 100 requests in a tight loop and require every one to fail | 20 |
| `database_contains_no_raw_token` — after pairing, dump every column of every table and require the raw token value to appear **nowhere**, in any encoding (raw, hex, base64) | — |
| `token_digest_known_vector` — `SHA-256` of a fixed token equals a committed literal, so a refactor cannot silently change the verifier | — |
| `verifier_comparison_is_constant_time` — the comparison goes through `subtle::ConstantTimeEq`; a source gate forbids `==` on the digest type | — |

### 4.2a Binding-profile tests (criterion 15, and the browser's future)

These exist because the web client arrives later and must not need a different
protocol. They test the *mechanism*, not the unimplemented browser profile.

| Test | Asserts |
| --- | --- |
| `profile_is_in_the_transcript` | proofs computed under two different profile ids differ for otherwise identical inputs — the property that makes a second profile safe to add |
| `armed_secret_permits_only_the_native_profile` | `atriumctl pair` arms `native-tls-exporter-v1`; a `begin` naming `web-pki-v1` is refused with the generic `pairing_rejected` |
| `profile_cannot_be_negotiated` | there is no code path in which the server's accepted profile set comes from the request; a source gate asserts the set is read from `pairing_state` only |
| `web_profile_is_not_implemented_in_m1` | requesting it fails, and the constant exists only as a reserved identifier — so nothing can accidentally reach a zero-binding code path |
| `transcript_length_is_208_bytes` | 32 + 16 + 32 + 32 + 32 + 32 + 32, with each field offset asserted |

### 4.3 MITM pairing test (criterion 15) — the important one

A harness that terminates TLS in front of Core with **its own** key pair, forwards
application bytes to the real Core, and is given the **correct secret**:

1. Client pins the MITM's SPKI and computes `proofC` over the MITM's SPKI and the
   MITM's exporter.
2. The MITM forwards `proofC` upstream.
3. **Core must reject**: its own SPKI and its own exporter differ.
4. The MITM cannot synthesize a `proofS` the client accepts.

The test asserts both directions and also the degenerate case where the MITM
tries to substitute only the SPKI, or only the exporter.

### 4.4 Hostile-server test (criterion 16)

A server harness that returns a well-formed but wrong `proofS`. The client must
report `untrusted`, and the test asserts **nothing was written** — no keychain
entry, no config record, no token on disk.

### 4.5 Recovery

| Test | Criterion |
| --- | --- |
| `corrupt_db_enters_recovery_not_crash_loop` — truncate the file mid-page, start, assert `/healthz` reports `state: recovery` with a reason, and the process stays up | 30 |
| `recovery_reports_the_corruption_clearly` — `/healthz` and the diagnostics payload both name the failure; neither says "ok" | 30 |
| `recovery_has_no_state_changing_route` — enumerate the recovery router and assert **every** entry is a `GET`; then sweep every known `POST`, `PUT` and `DELETE` path from the normal router and require `503 internal.recovery_mode` | 30 |
| `recovery_has_no_pairing_route` — all three `/api/v1/pair/*` paths return `503`, on every method | 30 |
| `recovery_never_contacts_the_agent` — run recovery with the agent socket replaced by a listener that fails the test if anything connects; assert nothing connects for the life of the process | 30 |
| `recovery_diagnostics_field_allowlist` — the payload is compared against a literal allowlist; a new field fails the test until it is reviewed. Asserts absence of hostname, addresses, device data and audit content | 30, 32 |
| `recovery_router_is_a_strict_subset` — the recovery route set ⊂ the normal route set ∪ {nothing}; no route exists only in recovery | 30 |
| `local_restore_succeeds_and_normal_service_resumes` — `atriumctl restore --from <backup>`, then assert Core starts normally, the schema is intact, and a device paired **before** the corruption still authenticates | 30 |
| `restore_moves_the_corrupt_database_aside` — the damaged file is preserved, not deleted | 30 |
| `partial_identity_enters_recovery_without_regenerating` — delete `identity.json`, keep the key; assert the key is byte-identical afterwards | — |

### 4.6 Identity

| Test | Criterion |
| --- | --- |
| `spki_survives_certificate_reissue` — force a SAN change, capture `SHA-256(SPKI)` before and after, assert byte equality | 7, 18 |
| `spki_survives_service_restart` — capture the pin, restart Core, capture again, assert byte equality. Separate from the reissue test because they fail for different reasons | 7 |
| `spki_survives_reboot_and_upgrade` — the same assertion across a VM reboot and across an in-place binary upgrade | 7 |
| `reissue_changes_serial_and_sans` | 7 |
| `rotation_changes_spki_and_revokes_all_devices` | 19 |
| `rotation_preserves_server_id` | 19 |
| `migration_backup_failure_aborts_migration` — read-only backup directory → migration refuses and enters recovery | — |

---

## 5. Privileged integration tests (L4 — require root)

Run under `sudo cargo test -p atrium-agent --test privilege_boundary` on a Linux
host or container. **Marked `#[ignore]` by default** so unprivileged `cargo test`
stays green, and run explicitly in CI's privileged job and in the VM suite.

| Test | Requires | Criterion |
| --- | --- | --- |
| `agent_rejects_non_core_uid` — connect as `nobody`, assert refusal and a journal line with `accepted:false` | root, a second uid | 8 |
| `agent_accepts_core_uid` — connect as `atrium` | root, the `atrium` user | 8 |
| `agent_state_dir_is_root_only` — `sudo -u atrium ls /var/lib/atrium-agent` fails with `EACCES` | root | 41 |
| `agent_journal_and_core_audit_correlate` — issue N calls, assert N Agent journal lines and N audit rows sharing `request_id` | root | 33 |
| `core_cannot_open_runtime_socket` — as `atrium`, `connect("/var/run/docker.sock")` returns `EACCES`. **Fails the test if Docker is absent**, rather than passing vacuously | root to set up, Docker installed | 35 |
| `file_modes_and_owners` — every path in the layout table asserted with `stat`: `/etc/atrium` is `root:atrium 0750` (group traverse+read, no write); `tls.key`, `identity.json` and `secrets.key` are `root:atrium 0640`; the agent socket is `root:atrium 0660` | root | 6 |
| `core_can_read_the_identity_key` — as `atrium`, open `tls.key` for reading succeeds. The invariant is "read yes, write no", so the read half is asserted too | root to set up | 6 |
| `core_cannot_modify_the_identity_key` — as `atrium`: open for write, `O_TRUNC`, `unlink`, `rename` over it, and `chmod` — **all five must fail**, each asserted separately with the errno | root to set up | 6 |
| `core_cannot_create_in_the_identity_directory` — as `atrium`, creating `tls.key.new` and any other file in `/etc/atrium` fails with `EACCES`, which is what makes replace-by-rename impossible | root to set up | 6 |
| `core_cannot_write_the_identity_directory` — as `atrium`, attempt to create, replace and unlink `tls.key`, `identity.json` and a new file in `/etc/atrium`; **all must fail with `EACCES`**. This is the test for the security-review fix in the plan's 23 | root to set up | 6 |
| `core_cannot_replace_core_toml` — same, for the configuration file | root to set up | — |
| `init_identity_is_idempotent_and_refuses_to_overwrite` — running `atrium-core init-identity` twice leaves the key byte-identical and exits non-zero the second time | root | 7 |
| `atriumctl_leaves_no_root_owned_wal` — run `atriumctl pair` as root, assert `atrium.db-wal` and `-shm` are owned by `atrium` | root | — |

---

## 6. Systemd tests (L5 — require systemd)

Run in a systemd-enabled container (`debian:12` with systemd, or `systemd-nspawn`)
for speed, and repeated in the VM suite for the criteria that need a real boot.

| Test | Requires | Criterion |
| --- | --- | --- |
| `systemd-analyze verify` on all three units | systemd | — |
| `systemd-analyze security atrium-core.service` exposure score recorded and asserted not to regress between releases | systemd | — |
| `both_units_active_after_start` | systemd | 3 |
| `socket_permissions` — `/run/atrium/agent.sock` is `0660 root:atrium` | systemd | 6 |
| `core_process_groups_are_empty` — `id -nG atrium` empty **and** the running process's `/proc/<pid>/status` `Groups:` line empty | systemd | 34 |
| `core_runs_as_atrium` — `ps -o user= -p $(pidof atrium-core)` | systemd | 5 |
| `agent_has_no_tcp_listener` — `ss -lntp` shows nothing for the agent pid | systemd | 5 |
| `agent_stopped_degrades_correctly` — `systemctl stop atrium-agent.socket atrium-agent.service`, assert capabilities and that every read route still answers | systemd | 29, 40 |
| `core_restart_preserves_identity_and_devices` | systemd | 7 |
| `recovery_does_not_restart_loop` — corrupt the DB, restart, assert `NRestarts` stops incrementing | systemd | 30 |

---

## 7. VM acceptance (L6)

### 7.1 Matrix

| Distribution | Role | Practicality |
| --- | --- | --- |
| **Debian 12** | the criteria's named target; full 48-criterion run | required |
| **Debian 13** | full run | required |
| **Ubuntu Server 24.04 LTS** | full run | required |
| **Ubuntu Server 26.04 LTS** | full run | required if released and installable at M1I; otherwise recorded as **not verified**, never as passing |

Per [`PLATFORM-MATRIX.md`](PLATFORM-MATRIX.md), a target that has not actually
been run stays out of the Supported tier. No distribution is marked supported on
the strength of another one passing.

### 7.2 VM variants

| Variant | Why | Criteria |
| --- | --- | --- |
| **Base** — one NIC, no Docker, internet available | the default run | most |
| **Two NICs** — a second virtual adapter, both with addresses | proves nothing takes `addresses[0]` | 23 |
| **Docker present** — `docker.io` installed, no containers running | the runtime probe and the `EACCES` test | 35, 39 |
| **No container runtime** | the honest-unavailable path | 25 |
| **No gateway** — default route removed after install | proves no outbound connection | 48 |
| **Prototype present** — the Python agent installed and running | canary rehearsal before touching the real server | 42–45 |

### 7.3 Scenario tests

| Scenario | Steps | Criteria |
| --- | --- | --- |
| **Install** | Run the one command; capture stdout; assert the secret and every address are printed | 1 |
| **Install is well-behaved** | `find / -newer <marker>` and a unit-list, `nft list ruleset`, `iptables-save` diff before/after | 2, 44 |
| **Reboot** | `reboot`; after boot assert both units active, identity unchanged (SPKI equal), devices intact, API answers | 3, 7 |
| **IP change** | Change the VM's address (DHCP re-lease or static edit), wait for the 30-second poll, assert: certificate serial changed, SAN set changed, **SPKI byte-identical**, mDNS re-announced, the already-paired client reconnects with no prompt | 7, 18 |
| **Identity rotation** | `atriumctl rotate-identity`; assert SPKI changed, all devices revoked, pairing armed, and the client reports `untrusted` and offers re-pairing | 19 |
| **Corrupted state** | `dd` 4 KiB of zeros into the middle of `atrium.db`; restart; assert recovery mode, no restart loop, `/healthz` and redacted diagnostics reachable, **every state-changing route refused**, the Agent socket never touched; then restore from the console with `atriumctl restore` and assert normal service resumes with previously paired devices still working | 30 |
| **Agent stopped** | Stop both agent units; assert `agent_unreachable` and that every read route still answers; restart and assert recovery without a Core restart | 29, 40 |
| **No internet** | Remove the default route; run the entire acceptance suite; in parallel capture `ss -tnp` every second for 10 minutes and assert **no** outbound connection from either binary | 48 |
| **Metrics under load** | Start a known busy loop; read `/proc/stat` and `/proc/loadavg` from the test; compare with `/api/v1/system/metrics` within tolerance | 22 |
| **Filesystems** | Create a bind mount and a tmpfs; assert the bind is collapsed, the tmpfs is excluded, and real capacity matches `df` | 24 |
| **Uninstall** | `uninstall.sh --keep-data` then `--purge`; assert unit files gone, binaries gone, data kept then removed, user removed only on purge | 4 |
| **Upgrade and rollback** | Install vN, pair, upgrade to vN+1, assert identity/devices survive; roll back; assert the service starts and data is intact | 7 |

### 7.4 Discovery tests

Require **two hosts on one L2 segment** (two VMs on a host-only network, or a VM
plus the developer's machine).

| Test | Criteria |
| --- | --- |
| Client lists the server without being told its address | 9 |
| Manual address entry yields an identical `pair/info` and an identical pairing outcome | 10 |
| After an IP change, the client re-finds the same `server_id` and reconnects | 18 |
| A spoofed mDNS record advertising the real `server_id` with a wrong `spki` prefix does **not** cause the client to trust the impostor — pairing fails on the proof | 9 (negative) |
| With mDNS blocked (multicast dropped on the bridge), discovery reports `mdns_bind_failed`/no results and manual entry still works | — |

---

## 8. Canary tests (L7 — the owner's real server)

Run once, by hand, with a written log. **Nothing here is automated and nothing
here is destructive.**

1. Capture the baseline: `systemctl list-units --all`, `ss -lntp`,
   `ls -la /etc/sudoers.d/`, `ls -laR /etc/personal-hub-agent /var/lib/personal-hub-agent`,
   a `find / -xdev -newermt` marker.
2. Install Atrium.
3. Re-capture and diff. Assert: `personal-hub-agent.service` still active, 9473
   still bound by it, its user, sudoers file and state directory unchanged, and no
   file written outside Atrium's paths.
4. Assert `ss -lntp` shows exactly one Atrium listener, on 7443.
5. Exercise the prototype's desktop app: service tabs, health checks, Glances
   panel, Download Center. All must behave exactly as before.
6. Pair the desktop app's new canary panel; confirm the existing surfaces are
   unaffected.
7. Uninstall Atrium; re-diff; confirm the machine is back to its pre-install state
   and the prototype still works.

Criteria 42–45. The criteria explicitly require this on the owner's server, not
only in a VM.

---

## 9. Sanitization and hygiene checks

Run in CI on every commit, and again at M1I:

| Check | Fails on |
| --- | --- |
| Address scan | any RFC1918 literal outside the approved fixture set; any new public address |
| Secret scan | `password|token|secret|api[_-]?key` assigned a long literal outside the known negative-test sentinels |
| Personal path scan | `C:\Users\`, `/home/<name>`, `/Users/<name>`, `OneDrive` |
| Email scan | any address |
| **Log scan** | run a full pairing flow, capture stdout/stderr and the journal, and grep for the generated secret, its decoded bytes (hex and base64), the device token and both token hashes. **Any hit fails the build** |
| **Bundle scan** | generate a diagnostics bundle on a paired system and grep it for the same values, plus the TLS private key and `secrets.key` (criterion 32) |

The log and bundle scans are the ones that matter: they test the redaction rules
against reality rather than against intent.

---

## 10. Test requirements summary

| Requirement | Tests that need it |
| --- | --- |
| **root** | L4 privilege tests, file-mode assertions, Agent journal correlation, the `EACCES` runtime-socket test, `atriumctl` WAL-ownership test |
| **systemd** | every L5 test; unit verification; restart-loop assertions |
| **network namespaces** | optional isolation for the MITM harness and the no-gateway run; the VM variants make them unnecessary, but netns gives a faster inner loop for the MITM test |
| **a VM** | all of §7 — install, reboot, IP change, uninstall, upgrade, no-gateway, Docker-present |
| **a physical/virtual reboot** | criteria 3 and 7 — not simulated, not mocked |
| **multiple NICs** | criterion 23 — a second adapter, both configured |
| **two hosts on one L2 segment** | criteria 9, 10, 18's discovery half |
| **Docker installed** | criteria 35 and 39 — and the test must **fail** if Docker is absent rather than skip |
| **a Turkish locale** | criterion 12's locale assertion — `locale-gen tr_TR.UTF-8` in CI |
| **the owner's real server** | criteria 42–45 |

---

## 11. CI changes (specified, not made in this pass)

### 11.1 New job: `server`

`ubuntu-latest`, on every push and pull request:

```
- cargo fmt   --manifest-path server/Cargo.toml --check
- cargo clippy --manifest-path server/Cargo.toml --all-targets -- -D warnings
- locale-gen tr_TR.UTF-8                       # criterion 12
- cargo test  --manifest-path server/Cargo.toml
- cargo deny  --manifest-path server/Cargo.toml check
- bash server/ci/boundary-checks.sh            # criteria 36, 37, 38, 46, 47
- bash server/ci/sanitization-checks.sh        # §9
```

### 11.2 New job: `server-privileged`

`ubuntu-latest`, in a privileged step with systemd available (container with
systemd, or `sudo` plus a user fixture):

```
- create the atrium user and directories
- sudo -E cargo test --manifest-path server/Cargo.toml -- --ignored
- systemd-analyze verify server/packaging/systemd/*.{service,socket}
```

**As built (M1B).** The privileged tests run as a step of the `server` job, not
a separate job: the step creates the `atrium` user and group, asks cargo for the
already-built `privileged` test binary of `atriumctl`, and runs it with `sudo`
and `--ignored --test-threads=1`. Cargo itself never runs as root, so nothing
root-owned lands in the build tree. The suite builds a root-owned installation
under `/tmp` exactly as `install.sh` step 8 will, runs `init-identity` and Core
as the real `atrium` user, and makes every write attempt of criterion 6 from a
separate process running as `atrium`, asserting each errno. `systemd-analyze`
stays in its existing informational step. Test names match section 5 where the
test exists there; the recovery tests of section 4.5 that need an HTTP router
(`recovery_has_no_state_changing_route`, `recovery_has_no_pairing_route`,
`recovery_router_is_a_strict_subset`) arrive with the router in M1D, and the
diagnostics field allowlist is already tested against the payload type.

**As built (M1C).** The same step now runs two privileged binaries in turn:
`atriumctl`'s `privileged` and `atrium-agent`'s `privilege_boundary`. It also
requires `/usr/bin/systemd-socket-activate`, which hands the root Agent its
socket exactly as `atrium-agent.socket` does. The M1C tests of section 5
exist under the names given there: `agent_rejects_non_core_uid`,
`agent_accepts_core_uid`, `agent_state_dir_is_root_only` and
`agent_journal_and_core_audit_correlate`. The last one is in `atriumctl`'s
suite, because it needs a real installation and Core running as `atrium`.
The additions are `a_claimed_uid_in_the_request_changes_nothing`,
`hostile_frames_from_the_core_uid_are_refused`,
`core_cannot_read_forge_or_erase_the_journal` and
`core_without_agent_reports_unreachable_and_keeps_running`. Section 2.5's
codec tests are in `atrium-protocol` under the names listed there.
`core_cannot_open_runtime_socket` (criterion 35) stays with M1I, which owns
it in the plan.

**As built (M1D).** Section 4.1's tests are in `atrium-core/src/http/`. They
run the real listener on loopback, with a generated identity, a certificate
issued by M1B's code, and a client that pins the SPKI:

| Plan name | Where |
| --- | --- |
| `route_table_matches_expected_literal_list` | `http/routes.rs` |
| `every_response_carries_version_headers` | `assert_common_headers`, applied to every success and error in `http/tests.rs` |
| `unauthenticated_routes_return_401_problem` | `routing_is_exact_and_fails_closed` |
| `error_response_leaks_nothing` | `http/tests.rs` |
| `unknown_body_field_rejected`, `body_over_limit_rejected_before_parse` | `bodies_are_refused_or_bounded_before_parsing` (test-only route; M1E's DTOs below) |
| `host_header_allowlist` | `http/tests.rs`, plus the parser tests in `http/host.rs` |
| `origin_header_on_token_request_refused` | `cross_origin_browser_requests_are_refused_and_nothing_advertises_cors` |
| `recovery_has_no_state_changing_route`, `recovery_has_no_pairing_route`, `recovery_router_is_a_strict_subset` | `http/routes.rs` and `recovery_serves_only_health_and_redacted_diagnostics` |

These are added: TLS (identity key served, TLS 1.2 floor with EMS, no
resumption, plaintext gets no HTTP), the exporter (it matches the client's,
one context per connection, distinct across connections, TLS 1.3 only),
certificate reissue (same pin, new `Host` set), malformed and oversized
heads, the connection cap, the idle keep-alive bound and shutdown with
stalled clients. `atriumctl`'s privileged suite adds four end-to-end tests
with the real Core binary as `atrium`:
`core_serves_tls_with_the_identity_key_and_a_restart_keeps_the_pin`,
`the_exporter_never_reaches_the_log` (at debug level),
`recovery_serves_health_over_tls_and_never_contacts_the_agent` and
`core_stops_promptly_with_stalled_network_clients`. Each privileged
installation now listens on its own free loopback port.

### 11.3 New workflow: `m1-acceptance` (manual dispatch)

Builds the tarball, boots the distro VM matrix, runs §7. Not on every push —
it is slow — but required before M1 is declared done, and its log is the
"written test log" the criteria require.

### 11.4 Unchanged

The existing `frontend`, `rust` (Windows) and `agent` (Python) jobs keep their
current definitions. The Windows job additionally compiles `atrium-client`
through the new path dependency, which is why that crate is kept
platform-neutral and dependency-light.

---

**As built (M1E).** The pairing and device tests run the real listener with a
real database in a private temporary tree, arm pairing the way `atriumctl
pair` does (through a separate database connection), move a test clock, and
pair over real TLS 1.3 with a harness client built on `atrium-pairing` that
takes the exporter and the certificate from its own handshake.

| Plan name | Where |
| --- | --- |
| §2.1 codec tests (all eleven) and the 2 000-value round trip | `atrium-pairing/src/secret.rs` |
| `uppercasing_is_ascii_only_under_turkish_locale` | `atrium-pairing/tests/turkish_locale.rs` — re-runs itself under `LC_ALL=tr_TR.UTF-8`; fails instead of skipping under CI |
| §2.2 transcript and proofs, `profile_is_in_the_transcript`, `transcript_length_is_208_bytes` | `atrium-pairing/src/transcript.rs` (frozen vectors computed independently) |
| `token_digest_known_vector`, `verifier_comparison_is_constant_time` | `atrium-pairing/src/token.rs`; the no-`==` source gate |
| `happy_path_pairs_and_returns_token` | `http/pairing_tests.rs` |
| `proof_s_is_verifiable_by_the_client` | `proof_s_is_verifiable_by_the_client_and_a_wrong_one_is_refused`, and `atrium-pairing/src/client.rs` |
| `wrong_secret_returns_generic_rejection` | `wrong_secret_returns_generic_rejection_identical_to_every_other_refusal` (byte-identical to unknown attempt, web profile, unknown profile and expired secret) |
| `five_failures_lock_pairing` | `five_failures_lock_pairing_until_the_console_re_arms` |
| `used_secret_cannot_be_reused` | `used_secret_and_replayed_complete_are_refused` |
| `expired_secret_rejected`, `begin_complete_window_expires` | `expired_secret_is_refused_and_its_ciphertext_deleted`, `begin_complete_window_expires_after_two_minutes` |
| `complete_on_a_different_connection_is_refused` | `http/pairing_tests.rs` |
| `first_pairing_claims_the_server`, `second_pairing_without_a_fresh_secret_refused` | `first_pairing_claims_the_server_and_a_second_needs_a_fresh_secret` |
| `revocation_takes_effect_on_the_next_request`, `no_authentication_cache_exists` | `devices_authenticate_list_safely_and_revoke_immediately` (100 requests after revoking) and the no-cache source gate |
| `revoked_and_unknown_tokens_are_indistinguishable` | `revoked_unknown_and_malformed_tokens_are_indistinguishable` (status, body and header set; timing is not asserted) |
| `database_contains_no_raw_token` | `database_contains_no_raw_token_and_no_secret` |
| §4.2a `armed_secret_permits_only_the_native_profile`, `web_profile_is_not_implemented_in_m1`, `profile_cannot_be_negotiated` | the refusal-identity test above, `pairing::tests::the_armed_profile_comes_from_the_state_not_the_request`, the profile source gate |
| §4.3 MITM | `a_relaying_mitm_with_its_own_key_cannot_complete_pairing` — a real TLS-terminating relay with its own key and the correct secret, plus an attacker substituting only the SPKI or only the exporter |
| §4.4 hostile server | `a_hostile_server_with_a_wrong_proof_s_leaves_the_client_with_nothing` — the client's store stays empty; the same harness against the real server stores exactly a pin and a token |
| `unknown_body_field_rejected` (per DTO), `body_over_limit_rejected_before_parse` | `pairing_bodies_are_strict_bounded_and_json_only` |
| `recovery_has_no_pairing_route` | `recovery_has_no_pairing_route_on_any_method` |
| `local_restore_succeeds_and_normal_service_resumes` (the paired device) | `atriumctl` privileged `a_device_paired_before_corruption_authenticates_after_restore` |
| `rotation_changes_spki_and_revokes_all_devices` | `db::pairing::tests::an_identity_key_change_revokes_every_device_and_the_armed_secret`; `console` and `privileged` rotation tests (which also assert a fresh code is armed) |
| `atriumctl_leaves_no_root_owned_wal` | `atriumctl` privileged `pairing_end_to_end_…` |
| §9 log scan and bundle scan | `atriumctl` privileged `pairing_end_to_end_leaves_no_secret_in_logs_audit_diagnostics_or_responses` (Core at debug level, the audit table, `atriumctl diagnostics`, every response body, every state file) and `http/pairing_tests.rs::nothing_secret_reaches_the_audit_or_any_response` |

Added for ADR-019 (`pairing/seal.rs`, `pairing/tests.rs`, `db/pairing.rs`):
a frozen sealing vector computed independently; no plaintext in the database
or in a backup; another `secrets.key` cannot open; tampered ciphertext, nonce,
expiry or armed time refused and not counted; nothing recoverable after
consumption, expiry, lock or replacement; a Core restart inside the window
still pairs and an attempt begun before it does not; two valid attempts or one
attempt completed twice concurrently make exactly one device; a crash before
commit, or a failed device insert, leaves the secret armed and the server
unclaimed. Added for limits (`http/limits.rs`, `http/tests.rs`,
`http/pairing_tests.rs`): equivalent address forms are one source, one
source cannot hold every connection slot while another still gets in, four
sources fill the global cap exactly, the per-source and global buckets and
the bounded source table, attempts bounded to four with one per source, and
forwarding headers change nothing.

## 12. Coverage map — every criterion has a test

| # | Owning layer | Named test or scenario |
| --- | --- | --- |
| 1 | L6 | Install scenario, stdout capture |
| 2 | L6 | Install-is-well-behaved diff |
| 3 | L5 + L6 | `both_units_active_after_start`; Reboot scenario |
| 4 | L6 | Uninstall scenario |
| 5 | L5 | `core_runs_as_atrium`, `agent_has_no_tcp_listener` |
| 6 | L4 + L5 | `file_modes_and_owners`, `core_can_read_the_identity_key`, `core_cannot_modify_the_identity_key`, `core_cannot_create_in_the_identity_directory`, `socket_permissions` |
| 7 | L3 + L6 | `spki_survives_certificate_reissue`; Reboot, IP change, Upgrade |
| 8 | L4 | `agent_rejects_non_core_uid`, `agent_accepts_core_uid` |
| 9 | L6 | Discovery, two hosts |
| 10 | L6 | Discovery, manual entry equivalence |
| 11 | L3 + L6 | `happy_path_pairs_and_returns_token`; keychain assertion on Windows |
| 12 | L1 + L2 | The eleven codec tests, plus property tests |
| 13 | L3 | `wrong_secret_returns_generic_rejection`, `five_failures_lock_pairing` |
| 14 | L3 | `used_secret_cannot_be_reused`, `expired_secret_rejected`, `begin_complete_window_expires` |
| 15 | L1 + L3 | Binding-profile tests §4.2a, MITM harness §4.3 |
| 16 | L3 | Hostile-server harness, §4.4 |
| 17 | L3 | `first_pairing_claims_the_server`, `second_pairing_without_a_fresh_secret_refused` |
| 18 | L6 | IP-change scenario, client reconnect |
| 19 | L6 | Identity-rotation scenario, client copy assertion |
| 20 | L3 | `revocation_takes_effect_on_the_next_request` |
| 21 | L6 | `/system` assertions + dependency gate |
| 22 | L6 | Metrics-under-load scenario |
| 23 | L6 | Two-NIC variant |
| 24 | L6 | Filesystems scenario |
| 25 | L6 | No-container-runtime variant |
| 26 | L3 | `unauthenticated_routes_return_401_problem`, `error_response_leaks_nothing` |
| 27 | L3 | `every_response_carries_version_headers` |
| 28 | L1 + L3 | `unknown_body_field_rejected`, per-DTO |
| 29 | L5 | `agent_stopped_degrades_correctly` |
| 30 | L3 + L5 + L6 | Recovery suite (§4.5: no state-changing route, no pairing route, Agent never contacted, diagnostics allowlist, local restore, service resumes), `recovery_does_not_restart_loop`, Corrupted-state scenario |
| 31 | L1 | `error_codes_are_unique_and_stable`, `error_variants_are_exhaustive` |
| 32 | L6 + §9 | Diagnostics assertions + bundle scan |
| 33 | L4 | `agent_journal_and_core_audit_correlate` |
| 34 | L5 | `core_process_groups_are_empty` |
| 35 | L4 on the Docker-present VM | `core_cannot_open_runtime_socket` |
| 36 | CI | `boundary-checks.sh` dependency gate |
| 37 | L3 | `route_table_matches_expected_literal_list` |
| 38 | L1 + CI | Enum enumeration test + source gate |
| 39 | L6 | Docker-present variant |
| 40 | L5 | `agent_stopped_degrades_correctly` |
| 41 | L4 | `agent_state_dir_is_root_only` |
| 42 | L7 | Canary step 3 |
| 43 | L7 | Canary step 4 |
| 44 | L6 + L7 | Filesystem diff, both |
| 45 | L7 | Canary step 7 |
| 46 | L1 + L3 | Both enumeration tests |
| 47 | CI | Shell-call grep gate |
| 48 | L6 | No-gateway run with `ss` sampling |

**No criterion is proved only by a unit test where the criterion is about system
behaviour**, and none is left without an owner.

---

## 13. Exit condition

M1 is done when:

1. Every test above passes on **Debian 12**, and the full 48-criterion run is
   recorded in a written log.
2. The same run passes on Debian 13 and Ubuntu 24.04. Ubuntu 26.04 either passes
   or is recorded as **not verified** — never assumed.
3. Criteria 42–45 pass on the owner's real server, with the prototype still
   working afterwards.
4. CI is green, including the new `server` and `server-privileged` jobs.
5. The log and bundle secret scans are clean.

Partial credit does not exist.
