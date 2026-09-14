#!/usr/bin/env python3
"""Narrow, opt-in Linux power agent. Python 3.10+, standard library only."""

from __future__ import annotations

import argparse
import copy
import hashlib
import hmac
import http.server
import json
import os
from pathlib import Path
import re
import signal
import socket
import ssl
import stat
import subprocess
import sys
import tempfile
import threading
import time
from typing import Callable
import uuid


HOST = "192.168.1.10"
PORT = 9473
AUTHORITY = f"{HOST}:{PORT}"
COUNTDOWN_MS = 30_000
MAX_OPERATIONS = 256
MAX_JOURNAL_BYTES = 131_072
MAX_BODY_BYTES = 1_024
MAX_HEADER_BYTES = 8_192
ID_RE = re.compile(r"[0-9a-f]{32}\Z")
TOKEN_RE = re.compile(r"[0-9a-fA-F]{64}\Z")
STATES = {"scheduled", "executing", "cancelled", "failed", "interrupted", "completed"}
ACTIVE_STATES = {"scheduled", "executing"}
ACTIONS = {"reboot", "shutdown"}
OPERATION_KEYS = {"id", "action", "state", "requestedAt", "executeAt", "bootId"}
COMMANDS = {
    "reboot": ("/usr/bin/sudo", "-n", "/usr/bin/systemctl", "reboot"),
    "shutdown": ("/usr/bin/sudo", "-n", "/usr/bin/systemctl", "poweroff"),
}


class AgentError(Exception):
    def __init__(self, code: str, status: int = 400):
        super().__init__(code)
        self.code = code
        self.status = status


def unix_ms() -> int:
    return time.time_ns() // 1_000_000


def linux_boot_id() -> str:
    value = Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
    if str(uuid.UUID(value)) != value:
        raise ValueError("invalid_boot_id")
    return value


def linux_uptime() -> int:
    value = float(Path("/proc/uptime").read_text(encoding="ascii").split()[0])
    return max(0, int(value))


def secure_read(path: Path, limit: int, *, secret: bool = True) -> bytes:
    """Do not follow final symlinks or accept a writable/broadly readable secret."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    with os.fdopen(descriptor, "rb") as handle:
        info = os.fstat(handle.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise ValueError("unsafe_file")
        if os.name == "posix":
            if info.st_uid not in {0, os.geteuid()} or info.st_mode & 0o022:
                raise ValueError("unsafe_file_permissions")
            if secret and info.st_mode & 0o007:
                raise ValueError("unsafe_secret_permissions")
            if secret and info.st_mode & 0o040:
                if info.st_gid not in {os.getegid(), *os.getgroups()}:
                    raise ValueError("unsafe_secret_group")
        data = handle.read(limit + 1)
        if len(data) > limit:
            raise ValueError("file_too_large")
        return data


def secure_directory(path: Path, *, state: bool = False) -> None:
    """Trusted ancestors prevent path replacement after permission checks."""
    if not path.is_absolute():
        raise ValueError("absolute_path_required")
    for current in (path, *path.parents):
        info = current.lstat()
        if not stat.S_ISDIR(info.st_mode):
            raise ValueError("unsafe_directory")
        if os.name == "posix":
            if info.st_uid not in {0, os.geteuid()} or info.st_mode & 0o022:
                raise ValueError("unsafe_directory_permissions")
    if state and os.name == "posix":
        info = path.stat()
        if info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError("state_directory_must_be_private")


def strict_json(data: bytes) -> object:
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate_key")
            result[key] = value
        return result

    def reject_constant(_value: str) -> None:
        raise ValueError("invalid_number")

    return json.loads(data.decode("utf-8"), object_pairs_hook=unique_object,
                      parse_constant=reject_constant)


def valid_operation(value: object) -> bool:
    if not isinstance(value, dict) or set(value) != OPERATION_KEYS:
        return False
    if not isinstance(value["id"], str) or not ID_RE.fullmatch(value["id"]):
        return False
    if not isinstance(value["action"], str) or value["action"] not in ACTIONS:
        return False
    if not isinstance(value["state"], str) or value["state"] not in STATES:
        return False
    for field in ("requestedAt", "executeAt"):
        if type(value[field]) is not int or not 0 <= value[field] <= 2**53 - 1:
            return False
    if value["executeAt"] - value["requestedAt"] != COUNTDOWN_MS:
        return False
    try:
        return isinstance(value["bootId"], str) and str(uuid.UUID(value["bootId"])) == value["bootId"]
    except ValueError:
        return False


def dispatch_power(action: str) -> None:
    # No user-controlled command, argv, shell, environment or executable path.
    subprocess.run(COMMANDS[action], check=True, timeout=10, shell=False,
                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL,
                   env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LANG": "C"})


class OperationStore:
    def __init__(self, journal: Path, boot_id: str, *, dry_run: bool = True,
                 now: Callable[[], int] = unix_ms,
                 monotonic: Callable[[], float] = time.monotonic,
                 dispatch: Callable[[str], None] = dispatch_power):
        self.journal = journal
        self.boot_id = boot_id
        self.dry_run = dry_run
        self.now = now
        self.monotonic = monotonic
        self.deadlines: dict[str, float] = {}
        self.dispatch = dispatch
        self.lock = threading.RLock()
        self.records: dict[str, dict] = {}
        self.storage_failed = False
        self._load_and_recover()

    def _load_and_recover(self) -> None:
        if not self.journal.exists():
            return
        document = strict_json(secure_read(self.journal, MAX_JOURNAL_BYTES))
        if not isinstance(document, dict) or set(document) != {"version", "operations"}:
            raise ValueError("invalid_journal")
        if type(document["version"]) is not int or document["version"] != 1:
            raise ValueError("invalid_journal_version")
        records = document["operations"]
        if not isinstance(records, list) or len(records) > MAX_OPERATIONS:
            raise ValueError("invalid_journal_records")
        active_count = 0
        for operation in records:
            if not valid_operation(operation) or operation["id"] in self.records:
                raise ValueError("invalid_journal_operation")
            active_count += operation["state"] in ACTIVE_STATES
            self.records[operation["id"]] = operation
        if active_count > 1:
            raise ValueError("invalid_journal_active_operations")
        changed = False
        recovered = copy.deepcopy(self.records)
        for operation in recovered.values():
            if operation["state"] == "scheduled":
                operation["state"] = "interrupted"
                changed = True
            elif operation["state"] == "executing":
                operation["state"] = ("completed" if operation["bootId"] != self.boot_id
                                      else "interrupted")
                changed = True
        if changed:
            self._commit(recovered)

    def _commit(self, records: dict[str, dict]) -> None:
        if self.storage_failed:
            raise AgentError("storage_unavailable", 503)
        data = json.dumps({"version": 1, "operations": list(records.values())},
                          separators=(",", ":"), allow_nan=False).encode("utf-8")
        temporary = None
        try:
            descriptor, temporary = tempfile.mkstemp(prefix=".operations-", dir=self.journal.parent)
            with os.fdopen(descriptor, "wb") as handle:
                os.chmod(temporary, 0o600)
                handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.journal)
            temporary = None
            if os.name == "posix":
                directory_fd = os.open(self.journal.parent, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    os.fsync(directory_fd)
                finally:
                    os.close(directory_fd)
        except OSError as error:
            # Fail closed after ANY persistence uncertainty. Restart reconciles disk.
            self.storage_failed = True
            raise AgentError("storage_unavailable", 503) from error
        finally:
            if temporary is not None:
                try:
                    os.unlink(temporary)
                except OSError:
                    pass
        self.records = records

    def get(self, operation_id: str) -> dict:
        with self.lock:
            operation = self.records.get(operation_id)
            if operation is None:
                raise AgentError("not_found", 404)
            return dict(operation)

    def active(self) -> dict | None:
        with self.lock:
            return next((dict(item) for item in self.records.values()
                         if item["state"] in ACTIVE_STATES), None)

    def create(self, operation_id: str, action: str, boot_id: str, dry_run: bool) -> dict:
        if not isinstance(operation_id, str) or not ID_RE.fullmatch(operation_id):
            raise AgentError("invalid_request")
        if not isinstance(action, str) or action not in ACTIONS:
            raise AgentError("invalid_request")
        if not isinstance(boot_id, str) or type(dry_run) is not bool:
            raise AgentError("invalid_request")
        with self.lock:
            if boot_id != self.boot_id or dry_run != self.dry_run:
                raise AgentError("server_state_changed", 409)
            if self.storage_failed:
                raise AgentError("storage_unavailable", 503)
            if operation_id in self.records:
                existing = self.records[operation_id]
                if existing["action"] != action:
                    raise AgentError("operation_conflict", 409)
                return dict(existing)
            if self.active() is not None:
                raise AgentError("operation_active", 409)
            requested_at = self.now()
            operation = {"id": operation_id, "action": action, "state": "scheduled",
                         "requestedAt": requested_at, "executeAt": requested_at + COUNTDOWN_MS,
                         "bootId": self.boot_id}
            updated = copy.deepcopy(self.records)
            while len(updated) >= MAX_OPERATIONS:
                del updated[next(iter(updated))]
            updated[operation_id] = operation
            self._commit(updated)
            self.deadlines = {operation_id: self.monotonic() + COUNTDOWN_MS / 1000}
            return dict(operation)

    def cancel(self, operation_id: str) -> dict:
        with self.lock:
            operation = self.get(operation_id)
            if operation["state"] == "cancelled":
                return operation
            if operation["state"] != "scheduled" or self.monotonic() >= self.deadlines[operation_id]:
                raise AgentError("too_late", 409)
            updated = copy.deepcopy(self.records)
            updated[operation_id]["state"] = "cancelled"
            self._commit(updated)
            self.deadlines.clear()
            return dict(updated[operation_id])

    def tick(self) -> None:
        with self.lock:
            if self.storage_failed:
                return
            operation = self.active()
            if operation is None or operation["state"] != "scheduled":
                return
            if self.monotonic() < self.deadlines[operation["id"]]:
                return
            updated = copy.deepcopy(self.records)
            updated[operation["id"]]["state"] = "executing"
            self._commit(updated)  # Must be durable BEFORE any privileged call.
            self.deadlines.clear()
            try:
                if not self.dry_run:
                    self.dispatch(operation["action"])
            except subprocess.TimeoutExpired:
                # The OS may already have accepted the request. Keep durable
                # executing to block another power operation until reconciled.
                return
            except (OSError, subprocess.CalledProcessError):
                updated = copy.deepcopy(self.records)
                updated[operation["id"]]["state"] = "failed"
                self._commit(updated)
                return
            if self.dry_run:
                updated = copy.deepcopy(self.records)
                updated[operation["id"]]["state"] = "completed"
                self._commit(updated)
            # Real success remains executing until a different Linux boot ID is observed.

    def interrupt_scheduled(self) -> None:
        with self.lock:
            updated = copy.deepcopy(self.records)
            changed = False
            for operation in updated.values():
                if operation["state"] == "scheduled":
                    operation["state"] = "interrupted"
                    changed = True
            if changed:
                self._commit(updated)
                self.deadlines.clear()


class AgentHTTPServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 8
    allow_reuse_address = False

    def __init__(self, context: ssl.SSLContext, store: OperationStore, token: str):
        self.store = store
        self.token = token
        self.slots = threading.BoundedSemaphore(8)
        super().__init__((HOST, PORT), AgentHandler)
        self.socket = context.wrap_socket(self.socket, server_side=True,
                                         do_handshake_on_connect=False)
        self.timeout = 0.5

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(5)
        return connection, address

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except Exception:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        def expire_request():
            try:
                request.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

        # Socket timeouts alone restart on each recv: also cap total request
        # lifetime so slow headers cannot occupy all cancellation connections.
        deadline = threading.Timer(5, expire_request)
        deadline.daemon = True
        deadline.start()
        try:
            super().process_request_thread(request, client_address)
        finally:
            deadline.cancel()
            self.slots.release()

    def handle_error(self, _request, _client_address):
        # Never dump request material, authentication headers or TLS details.
        pass


class AgentHandler(http.server.BaseHTTPRequestHandler):
    server_version = "PersonalHubAgent"
    sys_version = ""
    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *_args):
        pass

    def send_error(self, code, message=None, explain=None):
        self._json(code, {"error": "invalid_request"})

    def _json(self, status: int, value: object) -> None:
        payload = json.dumps(value, separators=(",", ":"), allow_nan=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(payload)
        self.close_connection = True

    def _gate(self) -> int:
        if len(self.path) > 128 or "?" in self.path or "#" in self.path:
            raise AgentError("invalid_request")
        if sum(len(key) + len(value) + 4 for key, value in self.headers.items()) > MAX_HEADER_BYTES:
            raise AgentError("request_too_large", 431)
        if len(self.headers) > 32:
            raise AgentError("request_too_large", 431)
        if self.headers.get_all("Host", []) != [AUTHORITY]:
            raise AgentError("invalid_host")
        if "Origin" in self.headers:
            raise AgentError("browser_request_forbidden", 403)
        authorization = self.headers.get_all("Authorization", [])
        if len(authorization) != 1 or not hmac.compare_digest(
                authorization[0].encode("utf-8"), ("Bearer " + self.server.token).encode("ascii")):
            raise AgentError("unauthorized", 401)
        if "Transfer-Encoding" in self.headers or "Expect" in self.headers:
            raise AgentError("invalid_request")
        lengths = self.headers.get_all("Content-Length", [])
        if len(lengths) > 1 or (lengths and not re.fullmatch(r"[0-9]{1,4}", lengths[0])):
            raise AgentError("invalid_request")
        length = int(lengths[0]) if lengths else 0
        if length > MAX_BODY_BYTES:
            raise AgentError("request_too_large", 413)
        if self.command != "POST" and length != 0:
            raise AgentError("invalid_request")
        return length

    def _route(self) -> None:
        try:
            length = self._gate()
            if self.command == "GET" and self.path == "/v1/status":
                self._json(200, {"version": 1, "serverId": "personal-hub-server",
                                 "bootId": self.server.store.boot_id,
                                 "uptimeSeconds": linux_uptime(),
                                 "dryRun": self.server.store.dry_run,
                                 "activeOperation": self.server.store.active()})
                return
            if self.command == "POST" and self.path == "/v1/operations":
                if self.headers.get_all("Content-Type", []) != ["application/json"] or not length:
                    raise AgentError("invalid_request")
                data = self.rfile.read(length)
                if len(data) != length:
                    raise AgentError("invalid_request")
                try:
                    request = strict_json(data)
                except (ValueError, UnicodeError, RecursionError) as error:
                    raise AgentError("invalid_request") from error
                if not isinstance(request, dict) or set(request) != {"id", "action", "bootId", "dryRun"}:
                    raise AgentError("invalid_request")
                self._json(200, self.server.store.create(request["id"], request["action"],
                                                       request["bootId"], request["dryRun"]))
                return
            match = re.fullmatch(r"/v1/operations/([0-9a-f]{32})", self.path)
            if match and self.command == "GET":
                self._json(200, self.server.store.get(match.group(1)))
                return
            if match and self.command == "DELETE":
                self._json(200, self.server.store.cancel(match.group(1)))
                return
            raise AgentError("not_found", 404)
        except AgentError as error:
            self._json(error.status, {"error": error.code})
        except (TimeoutError, OSError, ValueError):
            self._json(503, {"error": "temporarily_unavailable"})

    do_GET = _route
    do_POST = _route
    do_DELETE = _route

    def handle_expect_100(self):
        self._json(400, {"error": "invalid_request"})
        return False


def certificate_fingerprint(certificate: Path) -> str:
    pem = secure_read(certificate, 65_536, secret=False).decode("ascii")
    return hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()


def build_tls_context(certificate: Path, key: Path) -> ssl.SSLContext:
    secure_directory(certificate.parent)
    secure_directory(key.parent)
    secure_read(certificate, 65_536, secret=False)
    secure_read(key, 65_536)
    # CPython exposes the certificate decoder used by its own ssl tests. Refuse
    # startup if unavailable; never silently omit the fixed-IP SAN check.
    decoder = getattr(ssl._ssl, "_test_decode_cert", None)
    if decoder is None:
        raise ValueError("cpython_certificate_decoder_required")
    decoded = decoder(str(certificate))
    if ("IP Address", HOST) not in decoded.get("subjectAltName", ()):
        raise ValueError("certificate_ip_san_required")
    now = time.time()
    if not ssl.cert_time_to_seconds(decoded["notBefore"]) <= now < ssl.cert_time_to_seconds(decoded["notAfter"]):
        raise ValueError("certificate_not_current")
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(str(certificate), str(key))
    return context


def lock_state_directory(directory: Path) -> int:
    # Bind happens after journal recovery, so lock first: a second process must
    # never "recover" (interrupt) the first process's live operation journal.
    import fcntl

    descriptor = os.open(directory / "agent.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError("unsafe_lock_file")
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return descriptor
    except Exception:
        os.close(descriptor)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--certificate", type=Path, default=Path("/etc/personal-hub-agent/server.crt"))
    parser.add_argument("--key", type=Path, default=Path("/etc/personal-hub-agent/server.key"))
    parser.add_argument("--token-file", type=Path, default=Path("/etc/personal-hub-agent/token"))
    parser.add_argument("--state-dir", type=Path, default=Path("/var/lib/personal-hub-agent"))
    parser.add_argument("--allow-power", action="store_true", help="explicitly enable real reboot/poweroff")
    parser.add_argument("--print-certificate-fingerprint", action="store_true")
    args = parser.parse_args()
    if args.print_certificate_fingerprint:
        try:
            print(certificate_fingerprint(args.certificate))
            return 0
        except (OSError, ValueError, UnicodeError):
            print("Certificate fingerprint unavailable.", file=sys.stderr)
            return 1
    if sys.platform != "linux" or os.geteuid() == 0:
        print("Run on Linux as the dedicated unprivileged service account.", file=sys.stderr)
        return 1
    state_lock = None
    try:
        secure_directory(args.token_file.parent)
        secure_directory(args.state_dir, state=True)
        token = secure_read(args.token_file, 65).decode("ascii").removesuffix("\n")
        if not TOKEN_RE.fullmatch(token):
            raise ValueError("invalid_token_file")
        context = build_tls_context(args.certificate, args.key)
        state_lock = lock_state_directory(args.state_dir)
        store = OperationStore(args.state_dir / "operations.json", linux_boot_id(),
                               dry_run=not args.allow_power)
        server = AgentHTTPServer(context, store, token)
    except (OSError, ValueError, AgentError, UnicodeError, RecursionError):
        if state_lock is not None:
            os.close(state_lock)
        print("Agent startup refused: check installation, permissions, TLS and journal.", file=sys.stderr)
        return 1
    stopped = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_args: stopped.set())
    signal.signal(signal.SIGINT, lambda *_args: stopped.set())

    def scheduler() -> None:
        while not stopped.wait(0.25):
            try:
                store.tick()
            except AgentError:
                # Persistent failure locks out later dispatch; never retry a power call.
                print("Operation storage unavailable; power dispatch disabled until restart.", file=sys.stderr)

    worker = threading.Thread(target=scheduler, name="power-countdown", daemon=True)
    worker.start()
    print(f"Personal Hub agent listening on https://{AUTHORITY}; dry-run={store.dry_run}", flush=True)
    try:
        while not stopped.is_set():
            server.handle_request()
    finally:
        stopped.set()
        server.server_close()
        worker.join(timeout=11)
        try:
            store.interrupt_scheduled()
        except AgentError:
            pass
        os.close(state_lock)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
