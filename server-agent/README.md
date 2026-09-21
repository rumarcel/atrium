# Atrium restricted server agent — Phase 9

An optional **Linux server-side** companion for the Windows desktop app. It is
not needed for dashboards, Glances, Homarr, qBittorrent or normal service tabs.
It can only schedule **reboot** or **shutdown** on the one server address its
reviewed systemd unit supplies (`--host`). It cannot run
arbitrary commands, control the local Windows computer, or access Docker sockets.

The supplied service starts in **dry-run mode**. In that mode countdown and
cancellation work, but no power command is executed. Real power needs an explicit
administrator-reviewed sudoers installation **and** the `--allow-power` flag.
This repository does not install anything on the server automatically.

## Requirements and trust model

- A systemd-based Linux server with a static private IPv4 address. Set that
  address **once**, as `ATRIUM_AGENT_HOST` in `/etc/personal-hub-agent/agent.env`;
  both reviewed systemd units read it from there, so no address is written into a
  unit file or into the source. The examples below use `10.0.0.10`, which is a
  placeholder — substitute your server's own address. The agent refuses to start
  without it, and accepts only an RFC1918, loopback or link-local literal, which
  must match the certificate's IP SAN and the address enrolled in the desktop
  app. (RFC 5737 documentation addresses such as `192.0.2.10` cannot be used
  here: they are not private, so the agent rejects them.)
- CPython 3.10+ with OpenSSL support; no pip packages. Startup explicitly checks
  the certificate's IP SAN and validity using CPython's stdlib certificate decoder.
- A dedicated, unprivileged `personalhub-agent` user and private state directory.
  The process refuses to run as root or on Windows.
- HTTPS only, fixed TCP port `9473` (not configurable), TLS 1.2 or newer, a
  private certificate whose **IP SAN is that same address**, and a random 256-bit
  bearer token (64 hex characters).
- Desktop certificate enrollment must happen through a trusted channel. The
  desktop trusts the manually supplied private certificate, not arbitrary invalid
  certificates. Do not disable certificate/IP/expiry checks.
- Restrict the port in your existing firewall to your trusted desktop IP(s).
  Do not expose it to the internet, port-forward it, or put it behind a proxy.
  No CORS is enabled; all requests carrying an `Origin` header are rejected.

Possession of the token **and** access to the agent gives power-control authority.
Keep the key/token private and the service group exclusive to the agent account.
The account, its source files, systemd unit and sudoers must not be writable by
other non-administrative users. TLS does not protect a compromised endpoint.

## Manual installation (review on your Linux server first)

These are administrator instructions, not commands run by the desktop. Confirm
your distribution's `python3`, `sudo`, `systemctl` and `nologin` paths first.
The power argv are fixed in the source to `/usr/bin/sudo -n /usr/bin/systemctl`
followed by `reboot` or `poweroff`; change neither side of the allowlist casually.
The examples assume Debian/Ubuntu-style command paths and an unused account name.

1. Create the account and protected directories. Run from the `server-agent/`
   directory after transferring the reviewed files to the server:

   ```sh
   sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin personalhub-agent
   sudo install -d -o root -g root -m 0755 /opt/personal-hub-agent
   sudo install -d -o root -g personalhub-agent -m 0750 /etc/personal-hub-agent
   sudo install -d -o personalhub-agent -g personalhub-agent -m 0700 /var/lib/personal-hub-agent
   sudo install -o root -g root -m 0644 personal_hub_agent.py /opt/personal-hub-agent/personal_hub_agent.py
   sudo install -o root -g root -m 0644 personal_hub_inventory.py /opt/personal-hub-agent/personal_hub_inventory.py
   sudo install -o root -g root -m 0644 personal-hub-agent.service /etc/systemd/system/personal-hub-agent.service
   ```

   Then write the one piece of configuration both units read. Replace
   `10.0.0.10` with this server's own private IPv4 address:

   ```sh
   printf 'ATRIUM_AGENT_HOST=10.0.0.10
' | sudo tee /etc/personal-hub-agent/agent.env >/dev/null
   sudo chown root:personalhub-agent /etc/personal-hub-agent/agent.env
   sudo chmod 0640 /etc/personal-hub-agent/agent.env
   ```

   The units use `EnvironmentFile=` without a leading `-`, so a missing file or
   an unset value fails the service at startup rather than falling back to a
   guessed address.

2. Generate a unique private TLS key/certificate and token **on the server**.
   The token is written directly to its protected file, not printed. Do not paste
   these secrets into source control, support logs or chat:

   ```sh
   sudo openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 365 \
     -keyout /etc/personal-hub-agent/server.key \
     -out /etc/personal-hub-agent/server.crt \
     -subj '/CN=Atrium Server' \
     -addext 'subjectAltName=IP:10.0.0.10' \
     -addext 'extendedKeyUsage=serverAuth'
   sudo openssl rand -hex -out /etc/personal-hub-agent/token 32
   sudo chown root:personalhub-agent /etc/personal-hub-agent/server.key /etc/personal-hub-agent/server.crt /etc/personal-hub-agent/token
   sudo chmod 0640 /etc/personal-hub-agent/server.key /etc/personal-hub-agent/token
   sudo chmod 0644 /etc/personal-hub-agent/server.crt
   ```

   OpenSSL's private-key output is normally owner-only. Keep these steps in a
   protected root terminal and tighten the key permissions before starting the
   service. The parent directory is already inaccessible to unrelated users.

3. Verify the public certificate's IP and SHA-256 fingerprint out-of-band:

   ```sh
   sudo openssl x509 -in /etc/personal-hub-agent/server.crt -noout -ext subjectAltName -dates -fingerprint -sha256
   sudo -u personalhub-agent /usr/bin/python3 -B /opt/personal-hub-agent/personal_hub_agent.py --print-certificate-fingerprint
   ```

   Copy only the public `server.crt` into the desktop enrollment field. Transfer
   the token through your trusted local secret-entry workflow into Atrium's
   Windows-backed vault. **Never copy `server.key` to the desktop.** Confirm the
   fingerprint on the actual server, not from an unauthenticated network response.
   Changing this certificate later requires manual desktop re-enrollment.

4. Restrict the existing firewall to trusted desktop source addresses, then start
   the supplied **dry-run** service. No sudoers rule is needed yet:

   ```sh
   sudo systemctl daemon-reload
   sudo systemctl enable --now personal-hub-agent.service
   sudo systemctl status personal-hub-agent.service
   ```

   In Atrium verify the exact target, dry-run banner, connection, server
   uptime, countdown and cancellation. Let a dry-run complete once. It must show
   completed while the server boot ID/uptime remain unchanged. The application
   must not automatically turn off dry-run or deploy a privileged rule for you.

## Explicitly enable real server power later

Do this only after reviewing the host identity, protected installation and dry-run
behavior. An uncancelled live operation can interrupt streams, downloads and writes.
`systemctl` uses the normal graceful shutdown path; no force option is provided.

1. Review the two exact entries in `personal-hub-agent.sudoers`, verify the
   executable is root-owned and not writable by the agent, then validate/install:

   ```sh
   sudo visudo -cf personal-hub-agent.sudoers
   sudo install -o root -g root -m 0440 personal-hub-agent.sudoers /etc/sudoers.d/personal-hub-agent
   sudo visudo -c
   ```

2. While the service is still in dry-run, verify its **actual kernel flag** and
   inspect the dedicated account's sudo policy without executing a power command:

   ```sh
   agent_pid=$(systemctl show personal-hub-agent.service --property=MainPID --value)
   test "$agent_pid" -gt 1 && sudo awk '/^NoNewPrivs:/ {print}' "/proc/$agent_pid/status"
   sudo -u personalhub-agent /usr/bin/sudo -n -l
   ```

   The first check must report `NoNewPrivs: 0` for the running agent. The policy
   listing must allow only the two intended power argv, not unrestricted sudo.
   If either check fails, stop here and review host/unit policy; **do not grant
   `CAP_SYS_ADMIN`, run the agent as root, or broaden sudo privileges**.
   `systemctl show NoNewPrivileges` alone is insufficient because it may show the
   configured value rather than the kernel's implicitly enabled flag.

3. Use `sudo systemctl edit personal-hub-agent.service` to add this explicit
   administrator-controlled override:

   ```ini
   [Service]
   ExecStart=
   ExecStart=/usr/bin/python3 -B /opt/personal-hub-agent/personal_hub_agent.py --allow-power
   ```

4. Restart **the agent service only** with
   `sudo systemctl restart personal-hub-agent.service`. This command does not
   restart the host. Recheck the desktop target and live-mode banner before any
   power confirmation. Prepared dry-run requests are rejected after this mode
   change; the user must prepare/confirm again.

The unit deliberately has `NoNewPrivileges=false` because `sudo` needs privilege
elevation. Several systemd options can silently imply that flag under `User=`,
including `RestrictAddressFamilies`, `LockPersonality`, `RestrictRealtime`,
`RestrictSUIDSGID` and `ProtectKernel*`; therefore this template omits those
directives instead of granting extra capabilities. See the upstream
[systemd v249 execution/security documentation](https://raw.githubusercontent.com/systemd/systemd/v249/man/systemd.exec.xml).
The remaining hardening limits writable filesystem paths, home access and control
group writes and provides private temporary storage; it is **not** a full security sandbox. The
principal privileged boundary is the root-owned, exact-argv sudoers allowlist.
Do not add wildcards, shells, arbitrary unit names, `SETENV` or general sudo rights.
Do not enable unattended reboot/shutdown as an installation check.

## Protocol v1

All routes require `Host: 10.0.0.10:9473` and `Authorization: Bearer <64-hex-token>`.
Duplicate auth/host headers, query strings, browser Origin headers, request bodies
on GET/DELETE, transfer encoding and unexpected JSON keys are rejected. POST uses
exact `Content-Type: application/json`. No redirects, cookies or alternate routes.

| Method / route | Request / result |
| --- | --- |
| `GET /v1/status` | `{version:1,serverId:"personal-hub-server",bootId,uptimeSeconds,dryRun,activeOperation}` |
| `GET /v1/inventory` | Authenticated, optional sanitized rootful Docker/Podman snapshots and limited maintenance hint; see below |
| `POST /v1/operations` | Exact `{id,action,bootId,dryRun}` → operation, HTTP 200 |
| `GET /v1/operations/{id}` | Operation or HTTP 404 |
| `DELETE /v1/operations/{id}` | Cancel scheduled operation before deadline, or return existing cancelled record; otherwise HTTP 409 |

Operation: `{id,action,state,requestedAt,executeAt,bootId}`. IDs are exactly 32
lowercase hex characters; action is `reboot` or `shutdown`. Timestamps are Unix
milliseconds. States: `scheduled`, `executing`, `cancelled`, `failed`,
`interrupted`, `completed`. Status uptime is integer seconds from `/proc/uptime`,
not the desktop's uptime or the service's start time.

POST's `bootId` and boolean `dryRun` must match the server at scheduling time;
otherwise HTTP 409 `server_state_changed`. The desktop's two confirmations are
also bound to this identity/mode; the server is not a replacement for that UI.
Only one scheduled/executing operation exists at once. The delay is always 30
seconds; actual dispatch uses a monotonic timer so wall-clock jumps cannot
accelerate it. UTC `executeAt` is the display estimate. Cancellation and dispatch
use one lock; cancellation at/after the monotonic deadline is too late.

The journal is atomically replaced and fsynced **before** dispatch. A private
cross-process lock prevents a second agent from rewriting live state. A repeated
POST with an existing ID and the same action returns its recorded result without
rearming; a different action conflicts. Retention is the **latest 256 operations**;
IDs older than that may be accepted again. Clients must generate fresh random IDs
and never blindly retry an ambiguous old request, including after journal loss.

Scheduled operations are **interrupted**, never resumed, after an agent restart.
An executing operation becomes completed only if the Linux boot ID changed; if
the same boot remains it becomes interrupted. A successful real command stays
executing until that reconciliation. Explicit command failures become `failed`
and are never retried. A subprocess timeout may already have delivered the power
request: its durable state remains **executing**, blocking additional power
requests until manual reconciliation/agent restart or a boot transition. Inspect
host state before restarting the agent or issuing another operation.
`completed` after a new boot is
evidence of a boot transition, not proof of which actor caused it.

Journal corruption refuses startup; storage write failures disable dispatch until
restart. Do not delete the journal to clear errors while a power request might be
in flight. Preserve it for diagnosis and manually reconcile the host first.

Limits: 1 KiB bodies; 8 KiB/32 parsed headers; stdlib's parser additionally caps
individual header lines/count before this check; 128-character paths; eight
concurrent clients; five-second socket/overall request deadline; 10-second subprocess timeout;
128 KiB journal limit. Error replies are generic `{error:code}` with no secrets.
There is no request log, token echo, shell endpoint, Docker socket or local-machine
control API. The certificate/key/token and state directory are permission-checked.

## Phase 9.2.0 — Optional read-only rootful container inventory

The **network agent still has no container socket access**, Docker group, Docker
sudo rule, or container CLI execution. A separate, administrator-installed root
timer can run a fixed read-only list command and write sanitized snapshots. The
agent only reads these files. Neither a network request nor the desktop can run
the exporter, choose its command/path, restart a container, install an update, or
change this trust boundary. Existing power sudoers remains unchanged.

This first implementation covers **rootful Docker and rootful Podman only**.
Rootless Podman/Docker containers belong to a separate user context and are not
enumerated; an empty rootful result does **not** mean that user's containers are
absent. Rootless publication needs a separately designed, account-scoped trust
boundary and is deferred. Host-network containers and services with no published
IPv4 TCP mapping need manual configuration. Docker/Podman is never required for
the rest of Atrium.

### Manual opt-in installation

Review these files on the actual Linux host; these are **not** automatically run
by Atrium. Install the regular agent and its `personal_hub_inventory.py`
module first, including when upgrading a pre-inventory agent. Confirm the selected
runtime executable is root-owned at `/usr/bin/docker` or `/usr/bin/podman` and
that its normal **rootful** storage/configuration is the intended one.

```sh
sudo install -d -o root -g personalhub-agent -m 0750 /var/lib/personal-hub-inventory
sudo install -d -o root -g root -m 0700 /etc/personal-hub-inventory/docker
sudo install -o root -g root -m 0644 export_inventory.py /opt/personal-hub-agent/export_inventory.py
sudo install -o root -g root -m 0644 personal_hub_inventory.py /opt/personal-hub-agent/personal_hub_inventory.py
sudo install -o root -g root -m 0644 personal-hub-inventory@.service /etc/systemd/system/personal-hub-inventory@.service
sudo install -o root -g root -m 0644 personal-hub-inventory@.timer /etc/systemd/system/personal-hub-inventory@.timer
sudo systemctl daemon-reload
```

Keep the Docker configuration directory empty and root-owned. The exporter fixes
Docker's endpoint to `unix:///var/run/docker.sock`, explicitly disables Podman
remote mode, and does not inherit `DOCKER_HOST`, `DOCKER_CONTEXT`, `CONTAINER_HOST`
or a user's environment. There is no shell or arbitrary command argument. The
rootful Podman CLI may update its own runtime/database bookkeeping while listing;
the exporter performs no workload mutation, but is **not a security sandbox**.
Its scripts, unit, runtime binaries and configuration must remain administrator
controlled. Never grant these privileges to `personalhub-agent` itself.

Enable only the runtime(s) actually in use, after review:

```sh
# Rootful Docker (omit if unused):
sudo systemctl enable --now personal-hub-inventory@docker.timer
# Rootful Podman (omit if unused):
sudo systemctl enable --now personal-hub-inventory@podman.timer
```

Each timer refreshes around every 30 seconds. It writes only
`/var/lib/personal-hub-inventory/docker.json` or `podman.json` using atomic replace
and fsync, owned by `root:personalhub-agent` with mode `0640`. The agent cannot
write to their root-owned parent. Existing agent installations need a reviewed
**agent-service-only** restart to load the new route; no host reboot is needed.
Do not restart an agent with uncertain/in-flight power work just to upgrade it.

To disable an exporter, stop and disable its matching timer. Its last snapshot
expires after 180 seconds; until then it may still appear fresh. Remove only the
specific snapshot if immediate withdrawal is desired, after reviewing its path.
Do not remove the separate power-operation journal. No power permission or live
reboot/shutdown test is required to use inventory in dry-run mode.

### Data contract and limits

Authenticated `GET /v1/inventory` returns HTTP 200 with exact fields:

```json
{
  "version": 1,
  "target": "10.0.0.10",
  "sources": [
    {"runtime":"docker","scope":"rootful","state":"missing","ageSeconds":null,"skippedCount":0,"containers":[]},
    {"runtime":"podman","scope":"rootful","state":"missing","ageSeconds":null,"skippedCount":0,"containers":[]}
  ],
  "maintenance": {"rebootRequired": null}
}
```

There are always two sources, Docker then Podman. Source state is `ready`,
`missing`, `stale`, `unavailable`, or `invalid`. A source's containers appear only
when ready. Read failures, invalid metadata/permissions, timestamps more than 30
seconds in the future, snapshots older than 180 seconds, or a previous Linux boot
never yield import candidates. A missing exporter does not prevent agent startup
or power controls. Authentication errors still use the original HTTP error gate.

Each container is exactly `{id,name,application,state,ports}`. The ID is 12 lower
hex characters; name is at most 80 ASCII letters/digits/dots/underscores/hyphens,
starting with a letter or digit. State is `running`, `stopped` or `unknown`.
Application is a known built-in app key or null, inferred only from the image's
final repository component; it is a hint, **not verified application identity**.
Raw image names/tags, labels, environment, commands, mounts, registry information
and internal addresses are not exported. The list command requests only the five
fields needed for this reduction, not a full JSON/inspect dump.

Each port is `{hostPort,containerPort}` with integers 1–65535. Only explicitly
published TCP on `0.0.0.0` or `10.0.0.10` is admitted. Localhost, other host
addresses, IPv6-only binds, un-published internal ports, UDP and port-range strings
are omitted. Maximum: 64 containers per runtime, eight ports per container,
64 KiB per snapshot and less than 128 KiB aggregate. Skipped containers (including
invalid rows, repeated short IDs, and the count limit) are counted up to 50,000; omitted ports
are not counted. A non-ready source has no containers. CLI failures produce an
unavailable snapshot, never retain an older ready list or echo stderr.

The on-disk document is version 1 with exact
`{version,runtime,scope,bootId,generatedAt,state,skippedCount,containers}` fields;
`generatedAt` is Unix milliseconds and its state is `ready` or `unavailable`.
Root ownership, protected ancestor directories, regular-file type, no final
symlink, a single hardlink and bounded reads are enforced by the agent. Exports
are capped at 256 KiB of CLI stdout and an eight-second CLI deadline; the unit
also has a 12-second runtime limit. Errors never include raw CLI data.

`maintenance.rebootRequired` is true only when the regular `/run/reboot-required`
sentinel exists; otherwise it is null/unknown, **never false**. This does not
inspect package updates and does not mean a reboot is safe: active downloads,
streams, writes, other services and distro-specific maintenance remain unknown.
No update installation, Wake-on-LAN, service/container restart or automatic power
action is included in this phase. Discovery remains preview-and-review only;
the desktop does not automatically add imported services or treat image hints as
proof of HTTP protocol, authentication, or reachability.

CLI field references: [Docker container list](https://docs.docker.com/reference/cli/docker/container/ls/)
and [Podman ps](https://docs.podman.io/en/latest/markdown/podman-ps.1.html).
Host-specific CLI versions, systemd, permissions and rootful storage still need
manual read-only validation on the actual server; mocks do not prove deployment.

## Focused verification and maintenance

```sh
python3 -B -m unittest discover -s server-agent -p 'test_*.py'
```

Run that from the repository root. Tests use an in-memory fake HTTP connection,
temporary journals, injected clocks and mocked dispatch. They never open the
agent listener or invoke real reboot/shutdown. Linux systemd/sudo/TLS deployment
still needs the manual dry-run check on your actual server; unit tests do not
claim to verify those host-specific integrations.

To return to dry-run, remove only the `--allow-power` override using your normal
systemd administration workflow, reload and restart the agent. Remove its sudoers
entry if power access is no longer needed. To disable the integration, disable it
in Atrium and stop/disable `personal-hub-agent.service` on the server.
Closing the desktop or disabling integration does **not** cancel an operation
already accepted by the agent; cancel explicitly while its countdown permits it.
An agent service restart interrupts scheduled work, but an executing system power
request cannot reliably be undone. Renew certificates before expiration and rotate
compromised tokens manually on both endpoints; neither is auto-trusted on change.
