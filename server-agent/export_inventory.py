#!/usr/bin/env python3
"""Root-only, manual opt-in container inventory exporter. No listener or control API."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import tempfile
import time

import personal_hub_inventory as inventory


# Only these five fields are requested; never full inspect / JSON containing
# labels, mounts, environment, commands, registry credentials or network internals.
FORMAT = ('{"id":{{json .ID}},"name":{{json .Names}},"image":{{json .Image}},'
          '"state":{{json .State}},"ports":{{json .Ports}}}')
COMMANDS = {
    "docker": ("/usr/bin/docker", "--host", "unix:///var/run/docker.sock",
               "--config", "/etc/personal-hub-inventory/docker", "container", "ls",
               "--all", "--no-trunc", "--format", FORMAT),
    "podman": ("/usr/bin/podman", "--remote=false", "ps", "--all", "--no-trunc", "--format", FORMAT),
}
ENVIRONMENT = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LANG": "C", "LC_ALL": "C", "HOME": "/nonexistent"}
MAX_COMMAND_BYTES = 262_144
COMMAND_TIMEOUT = 8
PORT_RE = re.compile(r"(?:0\.0\.0\.0|192\.168\.1\.10):([0-9]{1,5})->([0-9]{1,5})/tcp\Z")
IMAGE_ALIASES = {"immich-server": "immich", "homeassistant": "home-assistant",
                 "portainer-ce": "portainer", "portainer-ee": "portainer",
                 "adguardhome": "adguard-home", "crafty-4": "crafty"}


def command_output(runtime: str) -> bytes:
    """Bound both memory and wall time; never inherit remote-context env vars."""
    command = COMMANDS[runtime]
    with subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.DEVNULL, shell=False, env=ENVIRONMENT,
                          start_new_session=True) as process:
        output = bytearray()
        deadline = time.monotonic() + COMMAND_TIMEOUT
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise ValueError("command_timeout")
                    if not selector.select(remaining):
                        raise ValueError("command_timeout")
                    chunk = os.read(process.stdout.fileno(), min(8_192, MAX_COMMAND_BYTES + 1 - len(output)))
                    if not chunk:
                        break
                    output.extend(chunk)
                    if len(output) > MAX_COMMAND_BYTES:
                        raise ValueError("command_output_limit")
            if process.wait(timeout=max(0.001, deadline - time.monotonic())) != 0:
                raise ValueError("command_unavailable")
        except BaseException:
            # End a stalled CLI and any helper it may have spawned; never leave a
            # child holding stdout open after a failed timer invocation.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            raise
        return bytes(output)


def application_for_image(value: object) -> str | None:
    if not isinstance(value, str) or len(value) > 1_024:
        return None
    leaf = value.rsplit("/", 1)[-1].split("@", 1)[0].split(":", 1)[0].lower()
    application = IMAGE_ALIASES.get(leaf, leaf)
    return application if application in inventory.APPLICATIONS else None


def safe_ports(value: object) -> list[dict]:
    if not isinstance(value, str) or len(value) > 16_384:
        return []
    ports = set()
    for binding in value.split(","):
        match = PORT_RE.fullmatch(binding.strip())
        if match:
            pair = tuple(int(part) for part in match.groups())
            if all(1 <= part <= 65_535 for part in pair):
                ports.add(pair)
    return [{"hostPort": host, "containerPort": container}
            for host, container in sorted(ports)[:inventory.MAX_PORTS]]


def sanitized_container(value: object) -> dict | None:
    if not isinstance(value, dict) or set(value) != {"id", "name", "image", "state", "ports"}:
        return None
    container_id = value["id"]
    if not isinstance(container_id, str) or not re.fullmatch(r"[0-9a-f]{12,64}", container_id):
        return None
    name = value["name"]
    # Podman versions may expose .Names as one-element arrays under JSON formatting.
    if isinstance(name, list) and len(name) == 1:
        name = name[0]
    if not isinstance(name, str) or not inventory.NAME_RE.fullmatch(name):
        return None
    state = value["state"]
    if not isinstance(state, str):
        return None
    normalized = "running" if state == "running" else (
        "stopped" if state in {"exited", "stopped", "created", "dead", "initialized"} else "unknown")
    return {"id": container_id[:12], "name": name, "application": application_for_image(value["image"]),
            "state": normalized, "ports": safe_ports(value["ports"])}


def export_document(runtime: str, boot_id: str, *, now: int | None = None) -> dict:
    result = {"version": 1, "runtime": runtime, "scope": "rootful", "bootId": boot_id,
              "generatedAt": time.time_ns() // 1_000_000 if now is None else now,
              "state": "unavailable", "skippedCount": 0, "containers": []}
    try:
        raw = command_output(runtime)
        if len(raw) > MAX_COMMAND_BYTES:
            raise ValueError("command_output_limit")
        seen = set()
        for line in raw.splitlines():
            if not line.strip():
                continue
            item = sanitized_container(inventory.strict_json(line))
            if item is None or item["id"] in seen or len(result["containers"]) >= inventory.MAX_CONTAINERS:
                result["skippedCount"] = min(inventory.MAX_SKIPPED, result["skippedCount"] + 1)
                continue
            seen.add(item["id"])
            result["containers"].append(item)
        result["containers"].sort(key=lambda item: item["id"])
        result["state"] = "ready"
    except (OSError, ValueError, UnicodeError, RecursionError, subprocess.SubprocessError):
        # No stale carry-over and no CLI output/errors in snapshots or logs.
        result.update(state="unavailable", skippedCount=0, containers=[])
    return result


def write_document(document: dict, group_id: int) -> None:
    directory = inventory.SNAPSHOT_DIRECTORY
    inventory.root_directory(directory)
    payload = json.dumps(document, separators=(",", ":"), allow_nan=False).encode("utf-8")
    inventory.parse_snapshot(payload, document["runtime"])
    temporary = None
    try:
        descriptor, temporary = tempfile.mkstemp(prefix=".inventory-", dir=directory)
        with os.fdopen(descriptor, "wb") as handle:
            os.fchown(handle.fileno(), 0, group_id)
            os.fchmod(handle.fileno(), 0o640)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, directory / f"{document['runtime']}.json")
        temporary = None
        descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if temporary is not None:
            os.unlink(temporary)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", required=True, choices=inventory.RUNTIMES)
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() != 0:
        print("Inventory export requires a reviewed root-owned Linux timer.", file=sys.stderr)
        return 1
    try:
        import grp

        document = export_document(args.runtime, inventory.current_boot_id())
        write_document(document, grp.getgrnam("personalhub-agent").gr_gid)
        return 0 if document["state"] == "ready" else 1
    except (OSError, ValueError, KeyError):
        print("Inventory export unavailable; check the reviewed installation.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
