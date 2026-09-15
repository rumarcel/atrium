"""Focused safety tests: no network listener and no real power subprocess."""

import io
import json
from pathlib import Path
import subprocess
import tempfile
import types
import unittest
from unittest import mock

import personal_hub_agent as agent


BOOT = "12345678-1234-4234-8234-123456789abc"
NEXT_BOOT = "abcdef01-1234-4234-8234-123456789abc"
ID = "a" * 32
SECOND_ID = "b" * 32
TOKEN = "c" * 64


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.journal = Path(self.directory.name) / "operations.json"
        self.wall = 1_700_000_000_000
        self.clock = 100.0
        self.dispatch = mock.Mock()
        self.store = self.make_store()

    def make_store(self, *, dry_run=True, boot_id=BOOT):
        return agent.OperationStore(self.journal, boot_id, dry_run=dry_run,
                                    now=lambda: self.wall, monotonic=lambda: self.clock,
                                    dispatch=self.dispatch)

    def create(self, operation_id=ID, action="reboot"):
        return self.store.create(operation_id, action, BOOT, self.store.dry_run)

    def advance(self):
        self.clock += 30
        self.wall += 30_000

    def test_dry_run_is_default_and_completes_without_subprocess(self):
        operation = self.create()
        self.assertEqual(operation["executeAt"] - operation["requestedAt"], 30_000)
        self.store.tick()
        self.assertEqual(self.store.get(ID)["state"], "scheduled")
        self.advance()
        self.store.tick()
        self.assertEqual(self.store.get(ID)["state"], "completed")
        self.dispatch.assert_not_called()

    def test_real_dispatch_is_durable_and_once_only(self):
        self.store = self.make_store(dry_run=False)
        self.create(action="shutdown")

        def check_durable(action):
            document = json.loads(self.journal.read_text(encoding="utf-8"))
            self.assertEqual(document["operations"][0]["state"], "executing")
            self.assertEqual(action, "shutdown")

        self.dispatch.side_effect = check_durable
        self.advance()
        self.store.tick()
        self.store.tick()
        self.assertEqual(self.store.get(ID)["state"], "executing")
        self.dispatch.assert_called_once_with("shutdown")

    def test_duplicate_post_never_rearms_or_changes_action(self):
        first = self.create()
        self.assertEqual(self.create(), first)
        self.store.cancel(ID)
        self.assertEqual(self.create()["state"], "cancelled")
        with self.assertRaises(agent.AgentError) as error:
            self.create(action="shutdown")
        self.assertEqual(error.exception.status, 409)

    def test_only_one_active_operation(self):
        self.create()
        with self.assertRaises(agent.AgentError) as error:
            self.create(SECOND_ID)
        self.assertEqual(error.exception.code, "operation_active")

    def test_cancellation_prevents_dispatch_and_is_idempotent(self):
        self.create()
        cancelled = self.store.cancel(ID)
        self.assertEqual(self.store.cancel(ID), cancelled)
        self.advance()
        self.store.tick()
        self.dispatch.assert_not_called()

    def test_expired_cancellation_is_rejected_before_scheduler_tick(self):
        self.create()
        self.advance()
        with self.assertRaises(agent.AgentError) as error:
            self.store.cancel(ID)
        self.assertEqual(error.exception.code, "too_late")

    def test_wall_clock_jump_cannot_accelerate_dispatch(self):
        self.create()
        self.wall += 3_600_000
        self.store.tick()
        self.assertEqual(self.store.get(ID)["state"], "scheduled")
        self.assertEqual(self.store.cancel(ID)["state"], "cancelled")

    def test_boot_and_dry_run_preconditions_prevent_mode_change(self):
        for boot, mode in ((NEXT_BOOT, True), (BOOT, False), (BOOT, 1)):
            with self.assertRaises(agent.AgentError):
                self.store.create(ID, "reboot", boot, mode)
        self.assertIsNone(self.store.active())

    def test_restart_interrupts_scheduled_operation_even_after_new_boot(self):
        self.create()
        self.advance()
        recovered = self.make_store(boot_id=NEXT_BOOT)
        recovered.tick()
        self.assertEqual(recovered.get(ID)["state"], "interrupted")
        self.dispatch.assert_not_called()

    def test_executing_requires_new_boot_to_be_completed(self):
        self.store = self.make_store(dry_run=False)
        self.create()
        self.advance()
        self.store.tick()
        original = self.journal.read_bytes()
        self.assertEqual(self.make_store().get(ID)["state"], "interrupted")
        self.journal.write_bytes(original)
        self.assertEqual(self.make_store(boot_id=NEXT_BOOT).get(ID)["state"], "completed")
        self.dispatch.assert_called_once()

    def test_persistence_failure_blocks_all_dispatch_until_restart(self):
        self.store = self.make_store(dry_run=False)
        self.create()
        self.advance()
        with mock.patch.object(agent.os, "replace", side_effect=OSError("mock disk failure")):
            with self.assertRaises(agent.AgentError):
                self.store.tick()
        self.store.tick()
        self.dispatch.assert_not_called()
        with self.assertRaises(agent.AgentError):
            self.create(SECOND_ID)

    def test_command_failure_is_terminal_and_never_retried(self):
        self.store = self.make_store(dry_run=False)
        self.dispatch.side_effect = subprocess.CalledProcessError(1, "mock")
        self.create()
        self.advance()
        self.store.tick()
        self.store.tick()
        self.assertEqual(self.store.get(ID)["state"], "failed")
        self.dispatch.assert_called_once()

    def test_command_timeout_keeps_uncertain_execution_and_blocks_new_power(self):
        self.store = self.make_store(dry_run=False)
        self.dispatch.side_effect = subprocess.TimeoutExpired("mock", 10)
        self.create()
        self.advance()
        self.store.tick()
        self.store.tick()
        self.assertEqual(self.store.get(ID)["state"], "executing")
        document = json.loads(self.journal.read_text(encoding="utf-8"))
        self.assertEqual(document["operations"][0]["state"], "executing")
        with self.assertRaises(agent.AgentError) as error:
            self.create(SECOND_ID)
        self.assertEqual(error.exception.code, "operation_active")
        self.dispatch.assert_called_once()

    def test_corrupt_journal_fails_closed(self):
        self.journal.write_text('{"version":1,"operations":[{"action":[]}]}', encoding="utf-8")
        with self.assertRaises(ValueError):
            self.make_store()

    def test_retention_is_bounded_and_keeps_recent_idempotence(self):
        with mock.patch.object(agent, "MAX_OPERATIONS", 3):
            for number in range(5):
                operation_id = f"{number:032x}"
                self.create(operation_id)
                self.store.cancel(operation_id)
        self.assertEqual(len(self.store.records), 3)
        self.assertNotIn("0" * 32, self.store.records)
        self.assertEqual(len(self.store.deadlines), 0)

    def test_dispatch_argv_is_fixed_and_shell_disabled(self):
        with mock.patch.object(agent.subprocess, "run") as run:
            agent.dispatch_power("reboot")
        self.assertEqual(run.call_args.args[0],
                         ("/usr/bin/sudo", "-n", "/usr/bin/systemctl", "reboot"))
        self.assertIs(run.call_args.kwargs["shell"], False)
        self.assertIs(run.call_args.kwargs["stdin"], subprocess.DEVNULL)


class FakeSocket:
    def __init__(self, request):
        self.input = io.BytesIO(request)
        self.output = bytearray()

    def makefile(self, _mode, _buffering):
        return self.input

    def sendall(self, data):
        self.output.extend(data)


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.store = agent.OperationStore(Path(self.directory.name) / "operations.json", BOOT)
        self.server = types.SimpleNamespace(store=self.store, token=TOKEN)

    def request(self, method="GET", path="/v1/status", *, token=TOKEN, host=agent.AUTHORITY,
                extra=(), body=b""):
        headers = [f"{method} {path} HTTP/1.1", f"Host: {host}", f"Authorization: Bearer {token}",
                   f"Content-Length: {len(body)}", *extra]
        connection = FakeSocket("\r\n".join(headers).encode("ascii") + b"\r\n\r\n" + body)
        with mock.patch.object(agent, "linux_uptime", return_value=42):
            agent.AgentHandler(connection, ("127.0.0.1", 1), self.server)
        header, content = bytes(connection.output).split(b"\r\n\r\n", 1)
        return int(header.split(b" ")[1]), json.loads(content)

    def test_authenticated_status_exact_shape(self):
        status, body = self.request()
        self.assertEqual(status, 200)
        self.assertEqual(body, {"version": 1, "serverId": "personal-hub-server", "bootId": BOOT,
                                "uptimeSeconds": 42, "dryRun": True, "activeOperation": None})
        self.assertNotIn(TOKEN, json.dumps(body))

    def test_inventory_uses_existing_auth_gate_and_no_power_dispatch(self):
        result = {"version": 1, "target": agent.HOST, "sources": [],
                  "maintenance": {"rebootRequired": None}}
        with mock.patch.object(agent, "inventory_response", return_value=result) as read:
            self.assertEqual(self.request(path="/v1/inventory"), (200, result))
            read.assert_called_once_with(BOOT)
            read.reset_mock()
            self.assertEqual(self.request(path="/v1/inventory", token="invalid")[0], 401)
            read.assert_not_called()
            self.assertEqual(self.request("POST", "/v1/inventory")[0], 404)
            self.assertEqual(self.request(path="/v1/inventory?runtime=docker")[0], 400)
            read.assert_not_called()
        self.assertIsNone(self.store.active())

    def test_host_origin_query_and_missing_authentication_are_rejected(self):
        cases = ({"host": "localhost:9473"}, {"extra": ("Origin: https://example.test",)},
                 {"path": "/v1/status?token=secret"}, {"token": "invalid"},
                 {"extra": (f"Authorization: Bearer {TOKEN}",)})
        for case in cases:
            with self.subTest(case=case):
                status, body = self.request(**case)
                self.assertGreaterEqual(status, 400)
                self.assertEqual(set(body), {"error"})
                self.assertNotIn(TOKEN, json.dumps(body))

    def test_strict_post_and_id_lookup(self):
        body = json.dumps({"id": ID, "action": "reboot", "bootId": BOOT, "dryRun": True}).encode()
        status, operation = self.request("POST", "/v1/operations",
                                         extra=("Content-Type: application/json",), body=body)
        self.assertEqual(status, 200)
        self.assertEqual(operation["state"], "scheduled")
        self.assertEqual(self.request(path=f"/v1/operations/{ID}")[1], operation)
        status, cancelled = self.request("DELETE", f"/v1/operations/{ID}")
        self.assertEqual(status, 200)
        self.assertEqual(cancelled["state"], "cancelled")

    def test_extra_missing_duplicate_and_non_boolean_fields_rejected(self):
        valid = {"id": ID, "action": "reboot", "bootId": BOOT, "dryRun": True}
        cases = [json.dumps({**valid, "command": "poweroff"}).encode(),
                 json.dumps({"id": ID, "action": "reboot"}).encode(),
                 json.dumps({**valid, "dryRun": 1}).encode(),
                 b'{"id":"a","id":"b"}']
        for body in cases:
            status, _ = self.request("POST", "/v1/operations",
                                     extra=("Content-Type: application/json",), body=body)
            self.assertEqual(status, 400)
        self.assertIsNone(self.store.active())

    def test_request_limits_and_encoded_paths_are_rejected(self):
        status, _ = self.request("POST", "/v1/operations", body=b"x" * 1_025)
        self.assertEqual(status, 413)
        status, _ = self.request(extra=("X-Oversized: " + "x" * 8_192,))
        self.assertEqual(status, 431)
        status, _ = self.request(path="/v1/%73tatus")
        self.assertEqual(status, 404)


if __name__ == "__main__":
    unittest.main()
