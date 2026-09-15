"""Read-only, bounded inventory bridge. This module never invokes a container CLI."""

from __future__ import annotations

import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import time
import uuid


ALLOWED_HOST_NETWORKS = tuple(ipaddress.IPv4Network(cidr) for cidr in (
    "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "127.0.0.0/8", "169.254.0.0/16"))
RUNTIMES = ("docker", "podman")
SNAPSHOT_DIRECTORY = Path("/var/lib/personal-hub-inventory")
MAX_SNAPSHOT_BYTES = 65_536
MAX_CONTAINERS = 64
MAX_PORTS = 8
MAX_SKIPPED = 50_000
MAX_INTEGER = 2**53 - 1
APPLICATIONS = frozenset({
    "jellyfin", "plex", "emby", "radarr", "sonarr", "bazarr", "prowlarr", "lidarr",
    "readarr", "immich", "navidrome", "audiobookshelf", "qbittorrent", "sabnzbd",
    "transmission", "metube", "homarr", "home-assistant", "nextcloud", "portainer",
    "glances", "grafana", "pihole", "adguard-home", "crafty", "gerbera",
})
ID_RE = re.compile(r"[0-9a-f]{12}\Z")
NAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,79}\Z")


def strict_json(data: bytes) -> object:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate_key")
            result[key] = value
        return result

    def reject(_value):
        raise ValueError("invalid_number")

    return json.loads(data.decode("utf-8"), object_pairs_hook=unique, parse_constant=reject)


def current_boot_id() -> str:
    value = Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
    if str(uuid.UUID(value)) != value:
        raise ValueError("invalid_boot_id")
    return value


def root_directory(path: Path) -> None:
    if not path.is_absolute():
        raise ValueError("absolute_directory_required")
    for current in (path, *path.parents):
        info = current.lstat()
        if not stat.S_ISDIR(info.st_mode):
            raise ValueError("unsafe_directory")
        if os.name == "posix" and (info.st_uid != 0 or info.st_mode & 0o022):
            raise ValueError("root_owned_directory_required")


def read_snapshot(runtime: str) -> bytes:
    # Neither the route nor its caller can supply a path or select another file.
    if runtime not in RUNTIMES:
        raise ValueError("invalid_runtime")
    root_directory(SNAPSHOT_DIRECTORY)
    descriptor = os.open(SNAPSHOT_DIRECTORY / f"{runtime}.json",
                         os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as handle:
        info = os.fstat(handle.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise ValueError("unsafe_snapshot")
        if os.name == "posix" and (info.st_uid != 0 or info.st_mode & 0o027):
            raise ValueError("unsafe_snapshot_permissions")
        data = handle.read(MAX_SNAPSHOT_BYTES + 1)
        if len(data) > MAX_SNAPSHOT_BYTES:
            raise ValueError("snapshot_too_large")
        return data


def valid_container(value: object) -> bool:
    if not isinstance(value, dict) or set(value) != {"id", "name", "application", "state", "ports"}:
        return False
    if not isinstance(value["id"], str) or not ID_RE.fullmatch(value["id"]):
        return False
    if not isinstance(value["name"], str) or not NAME_RE.fullmatch(value["name"]):
        return False
    if value["application"] is not None and (
            not isinstance(value["application"], str) or value["application"] not in APPLICATIONS):
        return False
    if not isinstance(value["state"], str) or value["state"] not in {"running", "stopped", "unknown"}:
        return False
    if not isinstance(value["ports"], list) or len(value["ports"]) > MAX_PORTS:
        return False
    seen = set()
    for port in value["ports"]:
        if not isinstance(port, dict) or set(port) != {"hostPort", "containerPort"}:
            return False
        if any(type(number) is not int or not 1 <= number <= 65_535 for number in port.values()):
            return False
        pair = (port["hostPort"], port["containerPort"])
        if pair in seen:
            return False
        seen.add(pair)
    return True


def parse_snapshot(data: bytes, runtime: str) -> dict:
    if runtime not in RUNTIMES:
        raise ValueError("invalid_runtime")
    if len(data) > MAX_SNAPSHOT_BYTES:
        raise ValueError("snapshot_too_large")
    value = strict_json(data)
    fields = {"version", "runtime", "scope", "bootId", "generatedAt", "state", "skippedCount", "containers"}
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError("invalid_snapshot")
    if type(value["version"]) is not int or value["version"] != 1:
        raise ValueError("invalid_version")
    if value["runtime"] != runtime or value["scope"] != "rootful":
        raise ValueError("invalid_runtime")
    if not isinstance(value["bootId"], str) or str(uuid.UUID(value["bootId"])) != value["bootId"]:
        raise ValueError("invalid_boot_id")
    if type(value["generatedAt"]) is not int or not 0 <= value["generatedAt"] <= MAX_INTEGER:
        raise ValueError("invalid_timestamp")
    if not isinstance(value["state"], str) or value["state"] not in {"ready", "unavailable"}:
        raise ValueError("invalid_state")
    if type(value["skippedCount"]) is not int or not 0 <= value["skippedCount"] <= MAX_SKIPPED:
        raise ValueError("invalid_skipped_count")
    containers = value["containers"]
    if not isinstance(containers, list) or len(containers) > MAX_CONTAINERS:
        raise ValueError("invalid_containers")
    if not all(valid_container(item) for item in containers):
        raise ValueError("invalid_container")
    if len({item["id"] for item in containers}) != len(containers):
        raise ValueError("duplicate_container")
    if value["state"] != "ready" and containers:
        raise ValueError("invalid_unavailable_snapshot")
    return value


def inventory_source(runtime: str, now: int, boot_id: str) -> dict:
    result = {"runtime": runtime, "scope": "rootful", "state": "missing", "ageSeconds": None,
              "skippedCount": 0, "containers": []}
    try:
        document = parse_snapshot(read_snapshot(runtime), runtime)
        difference = now - document["generatedAt"]
        if difference < -30_000:
            raise ValueError("future_snapshot")
        result["ageSeconds"] = max(0, difference // 1_000)
        if difference > 180_000 or document["bootId"] != boot_id:
            result["state"] = "stale"
            return result
        result.update(state=document["state"], skippedCount=document["skippedCount"],
                      containers=document["containers"])
    except FileNotFoundError:
        pass
    except (ValueError, UnicodeError, RecursionError):
        result["state"] = "invalid"
    except OSError:
        result["state"] = "unavailable"
    return result


def reboot_required() -> bool | None:
    # Absence is unknown: distributions need not provide this sentinel at all.
    try:
        root_directory(Path("/run"))
        info = Path("/run/reboot-required").lstat()
        if stat.S_ISREG(info.st_mode):
            return True
    except (OSError, ValueError):
        pass
    return None


def validate_host(value: str) -> str:
    """Accept only a bare private/loopback/link-local IPv4 literal.

    The desktop compares this against the address it enrolled and the agent's
    certificate must carry it as an IP SAN, so a hostname, port or URL here
    would make both checks ambiguous.
    """
    try:
        address = ipaddress.IPv4Address(value)
    except ipaddress.AddressValueError as error:
        raise ValueError("host_must_be_ipv4") from error
    # Exactly the ranges the desktop accepts (RFC1918, loopback, link-local).
    # `is_private` is deliberately not used: it also covers documentation,
    # carrier-grade NAT and benchmarking ranges, so the two sides would disagree
    # about which address may be enrolled.
    if not any(address in network for network in ALLOWED_HOST_NETWORKS):
        raise ValueError("host_must_be_private")
    return str(address)


def inventory_response(boot_id: str, host: str) -> dict:
    now = time.time_ns() // 1_000_000
    return {"version": 1, "target": host,
            "sources": [inventory_source(runtime, now, boot_id) for runtime in RUNTIMES],
            "maintenance": {"rebootRequired": reboot_required()}}
