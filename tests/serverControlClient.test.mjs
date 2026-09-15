import assert from "node:assert/strict";
import { test } from "node:test";
import { parseServerControlSnapshot } from "../node_modules/.cache/personal-hub-tests/features/serverControl/serverControlClient.js";
import { isPrivateIpv4 } from "../node_modules/.cache/personal-hub-tests/features/serverControl/serverControl.types.js";

function snapshot(overrides = {}) {
  return {
    target: "192.168.1.10:9473",
    address: "192.168.1.10",
    enabled: false,
    certificatePem: "",
    credentialStored: false,
    revision: "0",
    connectionState: "disabled",
    serverStatus: null,
    pendingConfirmation: null,
    activeOperation: null,
    history: [],
    notice: null,
    ...overrides,
  };
}

function operation(overrides = {}) {
  return {
    id: "operation-1",
    action: "reboot",
    state: "scheduled",
    requestedAt: 1_700_000_000_000,
    executeAt: 1_700_000_030_000,
    observedOffline: false,
    dryRun: true,
    ...overrides,
  };
}

function connected(overrides = {}) {
  return snapshot({
    enabled: true,
    // The IPC parser checks structure; native code validates the certificate.
    certificatePem: "-----BEGIN CERTIFICATE-----\nTEST-FIXTURE\n-----END CERTIFICATE-----",
    credentialStored: true,
    revision: "2",
    connectionState: "online",
    serverStatus: { bootId: "boot-1", uptimeSeconds: 120, dryRun: true },
    ...overrides,
  });
}

test("disabled server control accepts only its exact public contract", () => {
  assert.deepEqual(parseServerControlSnapshot(snapshot()), snapshot());
  const { credentialStored: omitted, ...incomplete } = snapshot();
  assert.equal(omitted, false);
  assert.throws(() => parseServerControlSnapshot(incomplete), /Unexpected server control fields/);
});

test("scheduled and uncertain operations retain their distinct states", () => {
  for (const state of ["scheduled", "uncertain"]) {
    const entry = operation({ state });
    const value = connected({ activeOperation: entry, history: [entry] });
    assert.deepEqual(parseServerControlSnapshot(value), value);
    assert.equal(parseServerControlSnapshot(value).activeOperation.state, state);
  }
});

test("server responses reject changed targets and credential fields at every boundary", () => {
  // The target must be exactly the enrolled address on the agent's fixed port.
  for (const mismatch of [
    { target: "127.0.0.1:9473" },
    { target: "192.168.1.10:9474" },
    { target: "192.168.1.10" },
    { target: "" },
    { address: "8.8.8.8", target: "8.8.8.8:9473" },
    { address: "example.com", target: "example.com:9473" },
    { address: "192.168.1.10", target: "192.168.0.14:9473" },
  ]) {
    assert.throws(() => parseServerControlSnapshot(snapshot(mismatch)), /Unexpected server control target/);
  }
  // Nothing enrolled yet is a valid state: both fields are empty together.
  const unenrolled = snapshot({ address: "", target: "" });
  assert.deepEqual(parseServerControlSnapshot(unenrolled), unenrolled);
  const pending = { id: "confirmation-1", action: "shutdown", expiresAt: 1_700_000_060_000, dryRun: true };
  for (const value of [
    snapshot({ token: "secret" }),
    connected({ serverStatus: { ...connected().serverStatus, password: "secret" } }),
    connected({ pendingConfirmation: { ...pending, token: "secret" } }),
    connected({ activeOperation: operation({ authorization: "secret" }) }),
    connected({ history: [operation({ secret: "secret" })] }),
  ]) {
    assert.throws(() => parseServerControlSnapshot(value), /Unexpected server control fields/);
  }
});

test("server responses reject unsafe numbers and malformed statuses", () => {
  for (const value of [
    connected({ serverStatus: { ...connected().serverStatus, uptimeSeconds: Number.MAX_SAFE_INTEGER + 1 } }),
    connected({ activeOperation: operation({ executeAt: Number.POSITIVE_INFINITY }) }),
    connected({ history: [operation({ requestedAt: -1 })] }),
    connected({ connectionState: "probably-online" }),
    connected({ activeOperation: operation({ state: "probably-completed" }) }),
    connected({ serverStatus: null }),
    connected({ credentialStored: false }),
  ]) {
    assert.throws(() => parseServerControlSnapshot(value));
  }
});

test("the enrolled address accepts only plain private IPv4 literals", () => {
  for (const valid of ["192.168.1.10", "192.168.1.10", "10.0.0.5", "172.16.4.2", "172.31.255.254", "127.0.0.1", "169.254.1.1"]) {
    assert.equal(isPrivateIpv4(valid), true, valid);
  }
  for (const invalid of [
    "", "8.8.8.8", "203.0.113.7", "172.15.0.1", "172.32.0.1", "example.com", "localhost",
    "192.168.1.10:9473", "https://192.168.1.10", "192.168.1.10/", " 192.168.1.10", "192.168.1.10 ",
    "192.168.0.256", "192.168.0", "192.168.0.1.1", "192.168.00.13", "0x c0.a8.0.13", "١٩٢.168.0.13",
  ]) {
    assert.equal(isPrivateIpv4(invalid), false, invalid);
  }
});
