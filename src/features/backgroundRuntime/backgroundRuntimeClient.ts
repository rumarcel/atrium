import { invoke, isTauri } from "@tauri-apps/api/core";
import { DESKTOP_WIDGET_KINDS } from "../desktopWidgets/desktopWidget.types.js";
import {
  BACKGROUND_RUNTIME_AVAILABILITIES,
  type BackgroundRuntimeAvailability,
  type BackgroundRuntimeClient,
  type BackgroundRuntimePreferences,
  type BackgroundRuntimeSnapshot,
  type DesktopCardAvailability,
  type DesktopCardPreferences,
  type SaveBackgroundRuntimePreferencesRequest,
} from "./backgroundRuntime.types.js";

const GET_PREFERENCES_COMMAND = "get_background_runtime_preferences";
const SAVE_PREFERENCES_COMMAND = "save_background_runtime_preferences";

export const BACKGROUND_RUNTIME_CHANGED_EVENT =
  "personal-hub://background-runtime-changed";
export const OPEN_SETTINGS_EVENT = "personal-hub://open-settings";
export const MAIN_RESUMED_EVENT = "personal-hub://main-resumed";

const SNAPSHOT_KEYS = new Set([
  "preferences",
  "revision",
  "availability",
  "trayAvailable",
  "recoveryNotice",
]);
const PREFERENCES_KEYS = new Set([
  "version",
  "experimentalDesktopCards",
  "closeToTray",
  "cards",
]);
const CARD_KEYS = new Set(DESKTOP_WIDGET_KINDS);

export const DEFAULT_BACKGROUND_RUNTIME_PREFERENCES: BackgroundRuntimePreferences =
  {
    version: 1,
    experimentalDesktopCards: false,
    closeToTray: true,
    cards: {
      server: true,
      storage: true,
      services: true,
    },
  };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertExactKeys(
  value: Record<string, unknown>,
  expected: ReadonlySet<string>,
  context: string,
): void {
  const unexpected = Object.keys(value).find((key) => !expected.has(key));
  const missing = [...expected].find(
    (key) => !Object.prototype.hasOwnProperty.call(value, key),
  );

  if (unexpected !== undefined || missing !== undefined) {
    throw new Error(`${context} had an unexpected response shape.`);
  }
}

function parseBoolean(value: unknown, field: string): boolean {
  if (typeof value !== "boolean") {
    throw new Error(`The background runtime response had an invalid ${field}.`);
  }

  return value;
}

function parseRevision(value: unknown): string {
  if (typeof value !== "string") {
    throw new Error("The background runtime response had an invalid revision.");
  }

  const revision = value.trim();
  if (revision.length === 0 || revision.length > 256 || revision !== value) {
    throw new Error("The background runtime response had an invalid revision.");
  }

  return revision;
}

function parseRecoveryNotice(value: unknown): string | null {
  if (value === null) {
    return null;
  }

  if (typeof value !== "string") {
    throw new Error(
      "The background runtime response had an invalid recovery notice.",
    );
  }

  const notice = value.trim();
  if (notice.length === 0 || notice.length > 1_000 || notice !== value) {
    throw new Error(
      "The background runtime response had an invalid recovery notice.",
    );
  }

  return notice;
}

function parseCards(value: unknown): DesktopCardPreferences {
  if (!isRecord(value)) {
    throw new Error("The background runtime response had invalid card settings.");
  }

  assertExactKeys(value, CARD_KEYS, "The background runtime card settings");
  return {
    server: parseBoolean(value.server, "server card setting"),
    storage: parseBoolean(value.storage, "storage card setting"),
    services: parseBoolean(value.services, "service-attention card setting"),
  };
}

export function parseBackgroundRuntimePreferences(
  value: unknown,
): BackgroundRuntimePreferences {
  if (!isRecord(value)) {
    throw new Error("The background runtime preferences were not an object.");
  }

  assertExactKeys(value, PREFERENCES_KEYS, "The background runtime preferences");
  if (value.version !== 1) {
    throw new Error("The background runtime preferences had an invalid version.");
  }

  return {
    version: 1,
    experimentalDesktopCards: parseBoolean(
      value.experimentalDesktopCards,
      "experimental desktop-card setting",
    ),
    closeToTray: parseBoolean(value.closeToTray, "close-to-tray setting"),
    cards: parseCards(value.cards),
  };
}

function parseAvailabilityValue(
  value: unknown,
): BackgroundRuntimeAvailability {
  const parsed = BACKGROUND_RUNTIME_AVAILABILITIES.find(
    (candidate) => candidate === value,
  );
  if (parsed === undefined) {
    throw new Error(
      "The background runtime response had invalid card availability.",
    );
  }

  return parsed;
}

function parseAvailability(value: unknown): DesktopCardAvailability {
  if (!isRecord(value)) {
    throw new Error(
      "The background runtime response had invalid card availability.",
    );
  }

  assertExactKeys(value, CARD_KEYS, "The background runtime card availability");
  return {
    server: parseAvailabilityValue(value.server),
    storage: parseAvailabilityValue(value.storage),
    services: parseAvailabilityValue(value.services),
  };
}

export function parseBackgroundRuntimeSnapshot(
  value: unknown,
): BackgroundRuntimeSnapshot {
  if (!isRecord(value)) {
    throw new Error("The native background runtime response was not an object.");
  }

  assertExactKeys(value, SNAPSHOT_KEYS, "The native background runtime response");
  return {
    preferences: parseBackgroundRuntimePreferences(value.preferences),
    revision: parseRevision(value.revision),
    availability: parseAvailability(value.availability),
    trayAvailable: parseBoolean(value.trayAvailable, "tray availability"),
    recoveryNotice: parseRecoveryNotice(value.recoveryNotice),
  };
}

function previewSnapshot(): BackgroundRuntimeSnapshot {
  return {
    preferences: parseBackgroundRuntimePreferences(
      DEFAULT_BACKGROUND_RUNTIME_PREFERENCES,
    ),
    revision: "preview",
    availability: {
      server: "available",
      storage: "available",
      services: "available",
    },
    trayAvailable: false,
    recoveryNotice:
      "Preview mode is read-only. Background cards require the desktop app.",
  };
}

function desktopOnlyError(): Error {
  return new Error(
    "Saving background runtime settings requires the Atrium desktop app.",
  );
}

export async function getBackgroundRuntimePreferences(): Promise<BackgroundRuntimeSnapshot> {
  if (!isTauri()) {
    return previewSnapshot();
  }

  const response = await invoke<unknown>(GET_PREFERENCES_COMMAND);
  return parseBackgroundRuntimeSnapshot(response);
}

export async function saveBackgroundRuntimePreferences(
  request: SaveBackgroundRuntimePreferencesRequest,
): Promise<BackgroundRuntimeSnapshot> {
  const normalizedRequest = {
    preferences: parseBackgroundRuntimePreferences(request.preferences),
    expectedRevision: parseRevision(request.expectedRevision),
  };
  if (!isTauri()) {
    throw desktopOnlyError();
  }

  const response = await invoke<unknown>(SAVE_PREFERENCES_COMMAND, {
    request: normalizedRequest,
  });
  return parseBackgroundRuntimeSnapshot(response);
}

export const nativeBackgroundRuntimeClient: BackgroundRuntimeClient = {
  getPreferences: getBackgroundRuntimePreferences,
  savePreferences: saveBackgroundRuntimePreferences,
};

export function describeBackgroundRuntimeError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.slice(0, 500);
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }

  return "The background runtime operation could not be completed.";
}
