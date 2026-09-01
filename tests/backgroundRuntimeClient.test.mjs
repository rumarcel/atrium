import assert from "node:assert/strict";
import { test } from "node:test";

import {
  DEFAULT_BACKGROUND_RUNTIME_PREFERENCES,
  parseBackgroundRuntimePreferences,
  parseBackgroundRuntimeSnapshot,
} from "../node_modules/.cache/personal-hub-tests/features/backgroundRuntime/backgroundRuntimeClient.js";
import { BackgroundRuntimeSaveGeneration } from "../node_modules/.cache/personal-hub-tests/features/backgroundRuntime/backgroundRuntimeSaveGeneration.js";
import { parseDesktopWidgetRuntimeState } from "../node_modules/.cache/personal-hub-tests/features/desktopWidgets/desktopWidgetClient.js";

function preferences(overrides = {}) {
  return {
    version: 1,
    experimentalDesktopCards: false,
    closeToTray: true,
    cards: {
      server: true,
      storage: true,
      services: true,
    },
    ...overrides,
  };
}

function snapshot(overrides = {}) {
  return {
    preferences: preferences(),
    revision: "v1:0",
    availability: {
      server: "available",
      storage: "glances-not-configured",
      services: "no-health-targets",
    },
    trayAvailable: true,
    recoveryNotice: null,
    ...overrides,
  };
}

test("background runtime snapshots accept only the exact bounded public shape", () => {
  assert.deepEqual(parseBackgroundRuntimeSnapshot(snapshot()), snapshot());

  assert.throws(
    () =>
      parseBackgroundRuntimeSnapshot({
        ...snapshot(),
        secret: "must never be accepted",
      }),
    /unexpected response shape/,
  );
  assert.throws(
    () =>
      parseBackgroundRuntimeSnapshot(
        snapshot({ revision: " revision-with-whitespace " }),
      ),
    /invalid revision/,
  );
  assert.throws(
    () => parseBackgroundRuntimeSnapshot(snapshot({ recoveryNotice: "" })),
    /invalid recovery notice/,
  );
  assert.throws(
    () => parseBackgroundRuntimeSnapshot(snapshot({ trayAvailable: "yes" })),
    /invalid tray availability/,
  );
});

test("background runtime preferences require every opt-in boolean and card key", () => {
  assert.deepEqual(
    parseBackgroundRuntimePreferences(DEFAULT_BACKGROUND_RUNTIME_PREFERENCES),
    DEFAULT_BACKGROUND_RUNTIME_PREFERENCES,
  );
  assert.equal(
    DEFAULT_BACKGROUND_RUNTIME_PREFERENCES.experimentalDesktopCards,
    false,
  );

  assert.throws(
    () => parseBackgroundRuntimePreferences(preferences({ version: 2 })),
    /invalid version/,
  );
  assert.throws(
    () =>
      parseBackgroundRuntimePreferences(
        preferences({ experimentalDesktopCards: "yes" }),
      ),
    /invalid experimental desktop-card setting/,
  );
  assert.throws(
    () =>
      parseBackgroundRuntimePreferences(
        preferences({ cards: { server: true, storage: true } }),
      ),
    /unexpected response shape/,
  );
  assert.throws(
    () =>
      parseBackgroundRuntimePreferences(
        preferences({
          cards: {
            server: true,
            storage: true,
            services: true,
            hidden: true,
          },
        }),
      ),
    /unexpected response shape/,
  );
});

test("card availability is allowlisted independently for all three cards", () => {
  for (const invalidAvailability of [
    { server: "maybe", storage: "available", services: "available" },
    { server: "available", storage: "available" },
    {
      server: "available",
      storage: "available",
      services: "available",
      extra: "available",
    },
  ]) {
    assert.throws(
      () =>
        parseBackgroundRuntimeSnapshot(
          snapshot({ availability: invalidAvailability }),
        ),
      /card availability|unexpected response shape/,
    );
  }
});

test("desktop widget runtime state exposes only a bounded geometry revision", () => {
  assert.deepEqual(parseDesktopWidgetRuntimeState({ geometryRevision: "7" }), {
    geometryRevision: "7",
  });
  assert.throws(
    () =>
      parseDesktopWidgetRuntimeState({
        geometryRevision: "7",
        path: "must-not-leak",
      }),
    /unexpected shape/,
  );
  assert.throws(
    () => parseDesktopWidgetRuntimeState({ geometryRevision: " 7" }),
    /invalid revision/,
  );
});

test("a runtime snapshot event does not mask the active local save failure", () => {
  const saves = new BackgroundRuntimeSaveGeneration();
  const activeSave = saves.begin();

  // Runtime snapshot events advance the independent response generation in
  // the component, but they do not supersede the local operation that caused
  // the event. Its eventual native reconciliation error must still surface.
  assert.equal(saves.isCurrent(activeSave), true);

  const newerSave = saves.begin();
  assert.equal(saves.isCurrent(activeSave), false);
  assert.equal(saves.isCurrent(newerSave), true);
});
