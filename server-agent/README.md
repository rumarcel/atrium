# Personal Hub restricted server agent — Phase 9

An optional **Linux server-side** companion for the Windows desktop app. It is
not needed for dashboards, Glances, Homarr, qBittorrent or normal service tabs.
It can only schedule **reboot** or **shutdown** on **192.168.1.10**. It cannot run
arbitrary commands, control the local Windows computer, or access Docker sockets.

The supplied service starts in **dry-run mode**. In that mode countdown and
cancellation work, but no power command is executed. Real power needs an explicit
administrator-reviewed sudoers installation **and** the `--allow-power` flag.
This repository does not install anything on the server automatically.

## Requirements and trust model

- A systemd-based Linux server at the fixed address `192.168.1.10`.
- CPython 3.10+ with OpenSSL support; no pip packages. Startup explicitly checks
  the certificate's IP SAN and validity using CPython's stdlib certificate decoder.
- A dedicated, unprivileged `personalhub-agent` user and private state directory.
  The process refuses to run as root or on Windows.
- HTTPS only, fixed TCP port `9473`, TLS 1.2 or newer, a private certificate with
  **IP SAN `192.168.1.10`**, and a random 256-bit bearer token (64 hex characters).
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
   sudo install -o root -g root -m 0644 personal-hub-agent.service /etc/systemd/system/personal-hub-agent.service
   ```

2. Generate a unique private TLS key/certificate and token **on the server**.
   The token is written directly to its protected file, not printed. Do not paste
   these secrets into source control, support logs or chat:

   ```sh
   sudo openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 365 \
     -keyout /etc/personal-hub-agent/server.key \
     -out /etc/personal-hub-agent/server.crt \
     -subj '/CN=Personal Hub Server' \
     -addext 'subjectAltName=IP:192.168.1.10' \
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
   the token through your trusted local secret-entry workflow into Personal Hub's
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

   In Personal Hub verify the exact target, dry-run banner, connection, server
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

All routes require `Host: 192.168.1.10:9473` and `Authorization: Bearer <64-hex-token>`.
Duplicate auth/host headers, query strings, browser Origin headers, request bodies
on GET/DELETE, transfer encoding and unexpected JSON keys are rejected. POST uses
exact `Content-Type: application/json`. No redirects, cookies or alternate routes.

| Method / route | Request / result |
| --- | --- |
| `GET /v1/status` | `{version:1,serverId:"personal-hub-server",bootId,uptimeSeconds,dryRun,activeOperation}` |
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
in Personal Hub and stop/disable `personal-hub-agent.service` on the server.
Closing the desktop or disabling integration does **not** cancel an operation
already accepted by the agent; cancel explicitly while its countdown permits it.
An agent service restart interrupts scheduled work, but an executing system power
request cannot reliably be undone. Renew certificates before expiration and rotate
compromised tokens manually on both endpoints; neither is auto-trusted on change.
