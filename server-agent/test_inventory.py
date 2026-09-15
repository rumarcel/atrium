"""Focused snapshot/metadata safety tests; no real CLI, listener or system writes."""

import json
import stat
import types
import unittest
from unittest import mock

import export_inventory as exporter
import personal_hub_inventory as inventory


HOST = "192.168.1.10"
PORT_RE = exporter.port_pattern(HOST)
BOOT = "12345678-1234-4234-8234-123456789abc"
OTHER_BOOT = "abcdef01-1234-4234-8234-123456789abc"
NOW = 1_700_000_000_000


def container():
    return {"id": "a" * 12, "name": "jellyfin", "application": "jellyfin", "state": "running",
            "ports": [{"hostPort": 8096, "containerPort": 8096}]}


def snapshot(**changes):
    return {"version": 1, "runtime": "docker", "scope": "rootful", "bootId": BOOT,
            "generatedAt": NOW, "state": "ready", "skippedCount": 0,
            "containers": [container()], **changes}


def raw_container(**changes):
    return {"id": "a" * 64, "name": "jellyfin", "image": "private.registry.test/team/jellyfin:latest",
            "state": "running", "ports": "0.0.0.0:8096->8096/tcp", **changes}


class InventoryReaderTests(unittest.TestCase):
    def source(self, document):
        with mock.patch.object(inventory, "read_snapshot", return_value=json.dumps(document).encode()):
            return inventory.inventory_source("docker", NOW, BOOT)

    def test_exact_ready_source_without_raw_metadata(self):
        self.assertEqual(self.source(snapshot()), {"runtime": "docker", "scope": "rootful", "state": "ready",
                         "ageSeconds": 0, "skippedCount": 0, "containers": [container()]})

    def test_stale_future_previous_boot_never_return_containers(self):
        for changed, expected in (({"generatedAt": NOW - 180_001}, "stale"),
                                  ({"generatedAt": NOW + 30_001}, "invalid"),
                                  ({"bootId": OTHER_BOOT}, "stale")):
            with self.subTest(changed=changed):
                result = self.source(snapshot(**changed))
                self.assertEqual(result["state"], expected)
                self.assertEqual(result["containers"], [])

    def test_missing_unavailable_invalid_distinguished(self):
        for error, state in ((FileNotFoundError(), "missing"), (PermissionError(), "unavailable"),
                             (ValueError(), "invalid")):
            with mock.patch.object(inventory, "read_snapshot", side_effect=error):
                self.assertEqual(inventory.inventory_source("docker", NOW, BOOT)["state"], state)
        self.assertEqual(self.source(snapshot(state="unavailable", containers=[]))["state"], "unavailable")

    def test_schema_bounds_duplicate_extra_keys_and_bool_numbers_rejected(self):
        bad_container = container()
        bad_container["ports"][0]["hostPort"] = True
        cases = [snapshot(secret="never"), snapshot(containers=[bad_container]),
                 snapshot(containers=[container()] * 65), snapshot(containers=[container()] * 2),
                 snapshot(state="unavailable"), snapshot(version=True), snapshot(scope="rootless"),
                 snapshot(containers=[{**container(), "application": "custom"}]),
                 snapshot(containers=[{**container(), "name": "bad\nname"}])]
        for document in cases:
            with self.subTest(document=document):
                self.assertEqual(self.source(document)["state"], "invalid")
        with self.assertRaises(ValueError):
            inventory.parse_snapshot(b'{"version":1,"version":1}', "docker")
        with self.assertRaises(ValueError):
            inventory.parse_snapshot(b" " * (inventory.MAX_SNAPSHOT_BYTES + 1), "docker")

    def test_aggregate_fixed_sources_and_unknown_maintenance(self):
        with mock.patch.object(inventory, "read_snapshot", side_effect=FileNotFoundError()), \
                mock.patch.object(inventory, "reboot_required", return_value=None):
            result = inventory.inventory_response(BOOT, HOST)
        self.assertEqual(set(result), {"version", "target", "sources", "maintenance"})
        self.assertEqual(result["target"], HOST)
        self.assertEqual([item["runtime"] for item in result["sources"]], ["docker", "podman"])
        self.assertEqual(result["maintenance"], {"rebootRequired": None})

    def test_nonregular_snapshot_is_opened_nonblocking_and_rejected(self):
        handle = mock.MagicMock()
        handle.fileno.return_value = 17
        info = types.SimpleNamespace(st_mode=stat.S_IFIFO | 0o640, st_nlink=1)
        with mock.patch.object(inventory, "root_directory"), \
                mock.patch.object(inventory.os, "O_NONBLOCK", 2048, create=True), \
                mock.patch.object(inventory.os, "open", return_value=17) as opened, \
                mock.patch.object(inventory.os, "fdopen") as fdopen, \
                mock.patch.object(inventory.os, "fstat", return_value=info):
            fdopen.return_value.__enter__.return_value = handle
            with self.assertRaises(ValueError):
                inventory.read_snapshot("docker")
            self.assertNotEqual(opened.call_args.args[1] & 2048, 0)
            handle.read.assert_not_called()


class InventoryExporterTests(unittest.TestCase):
    def test_only_explicit_fixed_ipv4_published_tcp_ports(self):
        value = ", ".join(["0.0.0.0:8096->8096/tcp", f"{HOST}:9443->443/tcp",
                           "127.0.0.1:9000->9000/tcp", "[::]:8080->80/tcp", "8080/tcp",
                           "192.168.0.14:9000->9000/tcp", "0.0.0.0:53->53/udp",
                           "0.0.0.0:0->80/tcp", "0.0.0.0:65536->80/tcp",
                           "0.0.0.0:8000-8009->8000-8009/tcp", "0.0.0.0:8096->8096/tcp"])
        self.assertEqual(exporter.safe_ports(value, PORT_RE), [{"hostPort": 8096, "containerPort": 8096},
                                                     {"hostPort": 9443, "containerPort": 443}])
        self.assertEqual(len(exporter.safe_ports(",".join(f"0.0.0.0:{port}->80/tcp" for port in range(8000, 8020)), PORT_RE)), 8)

    def test_leaf_classification_only_no_image_url_label_or_secret_output(self):
        raw = raw_container(image="private.example/team/jellyfin:secret-tag")
        actual = exporter.sanitized_container(raw, PORT_RE)
        self.assertEqual(actual, container())
        self.assertNotIn("private.example", json.dumps(actual))
        self.assertNotIn("secret-tag", json.dumps(actual))
        self.assertIsNone(exporter.application_for_image("private.example/jellyfin-not-official:v1"))
        self.assertEqual(exporter.application_for_image("ghcr.io/immich-app/immich-server:v1"), "immich")
        self.assertEqual(exporter.application_for_image("portainer/portainer-ce@sha256:abc"), "portainer")
        self.assertIsNone(exporter.sanitized_container({**raw, "labels": {"secret": "never"}}, PORT_RE))

    def test_bounded_snapshot_and_bad_rows_are_skipped(self):
        # Use distinct short IDs as emitted source IDs (not colliding zero prefixes).
        rows = [raw_container(id=f"{number:012x}" + "a" * 52) for number in range(70)]
        rows.append(raw_container(name="bad/name"))
        data = b"\n".join(json.dumps(row).encode() for row in rows)
        with mock.patch.object(exporter, "command_output", return_value=data):
            result = exporter.export_document("docker", BOOT, HOST, now=NOW)
        self.assertEqual(len(result["containers"]), 64)
        self.assertEqual(result["skippedCount"], 7)
        inventory.parse_snapshot(json.dumps(result).encode(), "docker")

    def test_cli_failure_and_malformed_output_do_not_reuse_or_echo_data(self):
        with mock.patch.object(exporter, "command_output", side_effect=OSError("SECRET")):
            result = exporter.export_document("docker", BOOT, HOST, now=NOW)
        self.assertEqual(result["state"], "unavailable")
        self.assertEqual(result["containers"], [])
        self.assertNotIn("SECRET", json.dumps(result))
        with mock.patch.object(exporter, "command_output", return_value=b"not-json SECRET"):
            self.assertEqual(exporter.export_document("podman", BOOT, HOST)["state"], "unavailable")

    def test_fixed_read_only_command_and_no_inherited_remote_context(self):
        for runtime, command in exporter.COMMANDS.items():
            self.assertEqual(command[0], f"/usr/bin/{runtime}")
            self.assertNotIn("inspect", command)
            self.assertNotIn("sudo", command)
            self.assertIn("--all", command)
            for disallowed in (".Labels", ".Mounts", ".Command", "{{json .}}"):
                self.assertNotIn(disallowed, command[-1])
        self.assertIn("unix:///var/run/docker.sock", exporter.COMMANDS["docker"])
        self.assertIn("--remote=false", exporter.COMMANDS["podman"])
        self.assertNotIn("DOCKER_HOST", exporter.ENVIRONMENT)
        self.assertNotIn("CONTAINER_HOST", exporter.ENVIRONMENT)


if __name__ == "__main__":
    unittest.main()


class HostValidationTests(unittest.TestCase):
    def test_host_accepts_only_plain_private_ipv4(self):
        for valid in ("192.168.1.10", "192.168.1.10", "10.0.0.5", "172.16.4.2", "127.0.0.1",
                      "169.254.1.1"):
            self.assertEqual(inventory.validate_host(valid), valid)
        # A hostname, port, URL or public address would make the desktop's
        # enrolled-address comparison and the certificate IP SAN ambiguous.
        for invalid in ("", "8.8.8.8", "203.0.113.7", "example.com", "localhost",
                        "192.168.1.10:9473", "https://192.168.1.10", "192.168.1.10/",
                        " 192.168.1.10", "192.168.1.256", "192.168.1", "::1", "0.0.0.0"):
            with self.assertRaises(ValueError, msg=invalid):
                inventory.validate_host(invalid)
