# UX states

Status: target contract between the API and every client.

The premise: a home server fails in ways a normal person cannot diagnose, and most
self-hosting interfaces respond either with a spinner that never ends or with a
stack trace. Atrium's job is to turn a technical failure into a sentence that says
what is wrong and what to do — while keeping the raw evidence one click away for
someone who wants it.

Two rules govern everything below.

1. **Every surface implements every applicable state.** A component that only has
   a success rendering is unfinished. Reviews check for this.
2. **Nothing is invented.** No placeholder numbers, no fabricated uptime, no
   optimistic success. If a value is unknown, the state is `unknown` and it says
   so. This rule is inherited verbatim from the prototype.

## The state set

| State | Meaning | Driven by |
| --- | --- | --- |
| `loading` | First data for this surface has not arrived yet | request in flight, no cached value |
| `stale` | Showing the last known value while a refresh is in flight or has failed | cached value + failed/pending refresh |
| `empty` | The request succeeded and there is genuinely nothing | `items: []` |
| `unconfigured` | The feature works but the user has not set it up | domain-specific |
| `unavailable` | This server cannot do this, for a stated reason | `capabilities.missing[]` |
| `degraded` | Partly working; some of it is missing or unreliable | capability `missing` + partial data |
| `permission-denied` | Authenticated, but this role may not | `auth.forbidden` |
| `disconnected` | The client cannot reach this server | transport failure |
| `untrusted` | Reached something, but it is not the paired server | SPKI pin mismatch |
| `update-required` | Client and Core are too far apart to proceed | capability negotiation |
| `in-progress` | An operation is running against this object | operation `queued`/`running` |
| `failed` | An operation finished unsuccessfully | operation `failed` + problem object |
| `partial` | An operation finished with mixed per-item results | operation `succeeded` + item outcomes |
| `unknown` | Atrium cannot determine the outcome | operation `unknown` |
| `not-ours` | The object exists but belongs to the user, not to Atrium | `managed: false` |
| `disowned` | Atrium created this app but can no longer prove it owns it | `container.ownership_mismatch` |
| `confirm-destructive` | Waiting for the owner to confirm something irreversible | confirmation flow |
| `recovering` | The platform is repairing itself | Core recovery mode |

## How each state behaves

### loading

Skeletons matching the real layout, not a centred spinner that causes a jump.
Never blocks the rest of the page — one slow panel does not hold the dashboard.
After a threshold (about 10 s) it becomes "still working" with a cancel, not an
indefinite spinner. It never silently becomes `empty`.

### stale

Show the last value, marked. "Last updated 2 minutes ago", with the refresh
control visible. Do not blank a working reading because one poll failed. This is
already the prototype's behaviour for metrics and it is correct.

### empty

Distinguish "nothing yet" from "nothing matches". Nothing yet gets the primary
action ("Install your first app"). Nothing matches gets the filter reset. Never
the same treatment as an error.

### unconfigured

A feature that exists but needs setup. Explains what it does, what it needs, and
offers the setup path. Critically: **a feature that is unconfigured must not
poll.** The prototype's rule for an absent Glances — no continuous request to an
endpoint that cannot exist — generalises to every integration.

### unavailable

The server genuinely cannot do this. The reason comes from
`capabilities.*.missing[].reason` and is rendered as a sentence:

| Reason code | Shown as |
| --- | --- |
| `no_container_runtime` | "No container runtime is installed on this server, so applications cannot run." |
| `not_permitted_in_alpha` | "This release does not allow applications to use the graphics hardware, so video is converted using the processor." |
| `not_enabled_in_alpha` | "Atrium cannot install system software in this release." |
| `no_smart_capable_device` | "None of this server's drives report SMART data." |
| `no_hwmon_sensors` | "This server does not expose temperature sensors." |
| `provider_unsupported` | "Atrium cannot do this on this operating system yet." |
| `agent_unreachable` | "The privileged helper is not running, so nothing that needs administrator rights can be done." |

Never a greyed-out control with no explanation, and never a hidden feature —
hidden is indistinguishable from broken.

### degraded

The most under-served state in this category of product and therefore the one to
get right. Some data is present, some is not; or the data is present but its
trustworthiness is reduced.

Rendering: show what exists, mark what is missing inline, and give the reason
once at panel level. A storage panel with four disks reporting and one refusing
SMART shows four healthy readings and one `unknown` — not a red panel, and not
four readings plus silence.

The prototype's `Local TLS` yellow state — reachable, but verified under an
explicit exception — is exactly this state done well, and stays.

### not-ours

A container the user created. Shown in the inventory with its name, image and
state, and with every control that would change it **absent, not disabled** —
because a disabled control implies it could be enabled, and this one cannot be
(ADR-013).

The explanation is one line and is not an apology: "This container was not created
by Atrium, so Atrium does not manage it." Where the product has a plan, it says
so: adopting existing containers is a future feature, not an oversight.

This state is easy to get wrong in two directions. Hiding these containers makes
Atrium look blind to a machine the user knows is busy. Showing them with greyed
start/stop buttons invites a support question every time. Show them, explain them
once, and move on.

### disowned

Rare and serious, and the user-facing half of the ownership boundary. Atrium
created this app, and then Agent found that the live container no longer matches
its own record — the owner token changed, the container was recreated out of band,
or something is impersonating it.

The interface says what is true and nothing more: Atrium can no longer confirm
that this container is the one it created, so it has stopped managing it. No
action is taken automatically, nothing is deleted, and there is no "fix it" button
that would amount to guessing. The offered paths are to inspect the container's
details, or to reinstall the app cleanly — the owner chooses.

The technical panel shows Agent's journal entry: what was recorded, what was
found, and when they diverged.

### permission-denied

Explain the role requirement, name the role that is needed, and offer the only
real action: ask the owner. Do not hide the control — a hidden control teaches
nothing. Do not show a control that produces a 403 on click either: render it
disabled with the reason.

### disconnected

The most common real failure. Three distinguishable causes, three messages:

| Cause | Message | Action |
| --- | --- | --- |
| No network on the client | "This device is offline." | retry |
| Server not reachable | "Atrium cannot reach *studyserver*. It may be asleep, rebooting, or on a different network." | retry, show last-known address, offer rediscovery |
| Server reachable, Core not responding | "*studyserver* answered, but Atrium is not running on it." | retry, show diagnostics guidance |

Behaviour: keep the last known state visible and marked `stale`, reconnect with
backoff, reconnect immediately on window focus or network change, and never lose
the user's place. Never a full-screen modal that discards the page.

### untrusted

Rare, serious, and never dismissible. The server at this address presented a
different key from the one paired with. Message: this is not the server you
paired with, and someone may be impersonating it. The only options are to stop, or
to re-pair deliberately with a fresh code. There is no "continue anyway".

### update-required

Client and Core disagree about the API. Says which side is behind, offers the
update path for the client, and — important — keeps read-only functionality
working where capability negotiation allows it, rather than presenting a wall.

### in-progress

Driven by the operation resource. Shows the step, not just a bar
("Downloading Jellyfin — step 3 of 6"). Cancellable when the operation says it
is; the control disappears when it stops being true. The object being operated on
is locked in the UI with its state shown, not hidden. Closing the window does not
cancel anything — the operation belongs to the server, and reopening rejoins it.

### failed

The core of this document. Every failure shows three layers:

1. **What happened**, in the user's terms — from `diagnosis`.
2. **What to do** — from `remediation`, as an action where one exists ("Choose a
   different port", "Free up space", "Retry").
3. **Why**, on demand — a collapsed "Technical details" panel containing `code`,
   `operationId`, `requestId`, the provider, the exit status, and a bounded
   excerpt of the underlying output. Copyable in one click.

Never: a raw error string as the headline. Never: "Something went wrong."
Never: a failure with no code.

### partial

Per-item outcomes, always. "6 of 8 containers restarted" with the two failures
listed and individually retryable. A partial result is never rolled up to
"succeeded" or "failed" — that is a lie in both directions.

### unknown

Atrium could not observe the outcome. Says so, says what it will do about it
("checking whether the server came back"), and offers no retry button for
anything dangerous. Reconciliation is automatic and its result replaces this
state. Reboot and shutdown are the obvious cases; a lost response to a container
stop is another.

### confirm-destructive

Names the object, names the consequences from the closed `consequences` enum, and
requires an action proportional to the damage:

| Severity | Confirmation |
| --- | --- |
| Interrupts service (restart, stop) | one click, clearly labelled |
| Deletes data (uninstall with data, remove a volume) | type the object's name |
| Affects the machine (shutdown, reboot, factory reset) | type the object's name plus a second review screen showing exactly what will happen |

The destructive action is never the default focus and never adjacent to a
routine one.

### recovering

Core is starting in a degraded mode — failed migration, corrupt state, missing
identity. The UI must still function enough to explain the situation, show
diagnostics, and offer the restore path. A recovery screen that cannot itself load
is a failure of this design.

## Expert affordances

The technical layer is always present and always consistent:

- A **Technical details** disclosure on every error, containing the problem
  object as shown in [`API.md`](API.md#4-error-model).
- **Copy diagnostics** on every failure, producing a redacted text block.
- **Operation history** per object, showing what was attempted, by which device,
  and how it ended.
- **Logs**, for any app or container, bounded and streamable.
- A **diagnostics bundle** for the whole system, redacted by allowlist.

Novices never encounter these unless they open them. Experts never have to leave
the interface to find out what actually happened. The product is not finished if
the honest answer to "why did that fail?" is "check the server's journal".

## Anti-patterns

- A spinner with no timeout.
- `empty` rendered when the request failed.
- A greyed control with no reason.
- A generic toast as the only report of a failed operation.
- A modal that destroys the page on a transient network blip.
- A retry button on an operation whose outcome is unknown and dangerous.
- Success reported before it has been observed.
- A number on screen that no provider actually returned.
