import assert from "node:assert/strict";
import { test } from "node:test";
import { parseServerControlSnapshot } from "../node_modules/.cache/personal-hub-tests/features/serverControl/serverControlClient.js";

function snapshot(overrides = {}) {
  return {
    target: "192.168.1.10:9473",
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
  assert.throws(() => parseServerControlSnapshot(snapshot({ target: "127.0.0.1:9473" })), /Unexpected server control target/);
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
