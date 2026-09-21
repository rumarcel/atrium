import assert from "node:assert/strict";
import { test } from "node:test";

import { createNativeServiceWebviewClient } from "../node_modules/.cache/personal-hub-tests/features/tabs/nativeServiceWebviewClient.js";

function harness() {
  const calls = [];
  let rejection = null;
  const client = createNativeServiceWebviewClient({
    isDesktopRuntime: () => true,
    invoke: async (command, payload) => {
      calls.push({ command, payload });

      if (rejection?.command === command) {
        const error = rejection.error;
        rejection = null;
        throw error;
      }

      if (command === "open_service_webview") {
        return { created: true };
      }

      if (command === "reconcile_service_webviews") {
        return { registeredServiceIds: [], message: null };
      }

      return undefined;
    },
  });

  return {
    calls,
    client,
    rejectNext(command, error) {
      rejection = { command, error };
    },
    resetCalls() {
      calls.length = 0;
    },
  };
}

function service(id) {
  return {
    id,
    name: id,
    description: "Test service",
    url: `http://10.0.0.10/${id}`,
    icon: "server",
    category: "Tests",
    enabled: true,
  };
}

const bounds = { x: 0, y: 100, width: 900, height: 600 };

test("an explicit tab close invokes the destructive close command", async () => {
  const { calls, client, resetCalls } = harness();
  const target = service("phase62-close");
  await client.showServiceWebview(target, bounds, true);
  resetCalls();

  await client.closeServiceWebview(target.id);

  assert.deepEqual(calls, [
    {
      command: "close_service_webview",
      payload: { serviceId: target.id },
    },
  ]);
  assert.equal("parkServiceWebview" in client, false);
});

test("switching among open tabs keeps warm views and never closes them", async () => {
  const { calls, client } = harness();
  const first = service("phase62-switch-a");
  const second = service("phase62-switch-b");

  await client.showServiceWebview(first, bounds, true);
  await client.showServiceWebview(second, bounds, true);
  await client.showServiceWebview(first, bounds, true);

  assert.deepEqual(
    calls.map(({ command }) => command),
    [
      "open_service_webview",
      "open_service_webview",
      "activate_service_webview",
    ],
  );
  assert.equal(
    calls.some(({ command }) => command === "close_service_webview"),
    false,
  );
});

test("a failed native close keeps the known warm view recoverable", async () => {
  const { calls, client, rejectNext, resetCalls } = harness();
  const target = service("phase62-close-failure");
  await client.showServiceWebview(target, bounds, true);
  resetCalls();
  rejectNext("close_service_webview", new Error("native close failed"));

  await assert.rejects(
    client.closeServiceWebview(target.id),
    /native close failed/,
  );
  await client.showServiceWebview(target, bounds, true);

  assert.deepEqual(
    calls.map(({ command }) => command),
    ["close_service_webview", "activate_service_webview"],
  );
});

test("a queued show cannot recreate a view after an immediate close", async () => {
  const { calls, client } = harness();
  const target = service("phase62-queued-close");

  const show = client.showServiceWebview(target, bounds, true);
  const close = client.closeServiceWebview(target.id);
  await Promise.all([show, close]);

  assert.deepEqual(
    calls.map(({ command }) => command),
    ["close_service_webview"],
  );
});

test("a show requested during close cannot reopen the renderer", async () => {
  const calls = [];
  let releaseClose;
  let markCloseStarted;
  const closeStarted = new Promise((resolve) => {
    markCloseStarted = resolve;
  });
  const closeGate = new Promise((resolve) => {
    releaseClose = resolve;
  });
  const client = createNativeServiceWebviewClient({
    isDesktopRuntime: () => true,
    invoke: async (command, payload) => {
      calls.push({ command, payload });

      if (command === "close_service_webview") {
        markCloseStarted();
        await closeGate;
      }

      return command === "open_service_webview" ? { created: true } : undefined;
    },
  });
  const target = service("phase62-show-during-close");
  await client.showServiceWebview(target, bounds, true);
  calls.length = 0;

  const close = client.closeServiceWebview(target.id);
  await closeStarted;
  const lateShow = client.showServiceWebview(target, bounds, false);
  releaseClose();
  await Promise.all([close, lateShow]);

  assert.deepEqual(
    calls.map(({ command }) => command),
    ["close_service_webview"],
  );
});

test("host teardown reconciles to an empty native registry", async () => {
  const { calls, client, resetCalls } = harness();
  const target = service("phase62-host-teardown");
  await client.showServiceWebview(target, bounds, true);
  resetCalls();

  await client.closeAllServiceWebviews();

  assert.deepEqual(calls, [
    {
      command: "reconcile_service_webviews",
      payload: { request: { enabledServiceIds: [] } },
    },
  ]);
});

test("host teardown retries the narrowed registry after a close failure", async () => {
  const calls = [];
  let attempt = 0;
  const client = createNativeServiceWebviewClient({
    isDesktopRuntime: () => true,
    invoke: async (command, payload) => {
      calls.push({ command, payload });

      if (command !== "reconcile_service_webviews") {
        return undefined;
      }

      attempt += 1;
      return attempt === 1
        ? {
            registeredServiceIds: ["phase62-retry-close"],
            message: "Could not release 1 native service view.",
          }
        : { registeredServiceIds: [], message: null };
    },
  });

  await client.closeAllServiceWebviews();

  assert.equal(attempt, 2);
  assert.deepEqual(
    calls.map(({ command }) => command),
    ["reconcile_service_webviews", "reconcile_service_webviews"],
  );
});
