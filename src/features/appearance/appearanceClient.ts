import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppearanceClient,
  AppearancePreferences,
  AppearanceSnapshot,
  ImportAppearancePreferencesRequest,
  ResetAppearancePreferencesRequest,
  SaveAppearancePreferencesRequest,
  ThemePackDocument,
} from "./appearance.types.js";
import {
  createDefaultAppearancePreferences,
  createThemePackDocument,
  parseAppearancePreferences,
  parseAppearanceRevision,
  parseAppearanceSnapshot,
  parseThemePackDocument,
} from "./appearanceSchema.js";

const GET_APPEARANCE_COMMAND = "get_appearance_settings";
const SAVE_APPEARANCE_COMMAND = "save_appearance_preferences";
const RESET_APPEARANCE_COMMAND = "reset_appearance_preferences";
const IMPORT_APPEARANCE_COMMAND = "import_appearance_preferences";
const EXPORT_APPEARANCE_COMMAND = "export_appearance_preferences";

export const APPEARANCE_CHANGED_EVENT = "personal-hub://appearance-changed";

function previewSnapshot(): AppearanceSnapshot {
  return {
    preferences: createDefaultAppearancePreferences(),
    revision: "preview",
    recoveryNotice:
      "Preview mode is read-only. Appearance changes require the desktop app.",
  };
}

function desktopOnlyError(action: string): Error {
  return new Error(`${action} requires the Personal Hub desktop app.`);
}

function normalizeSaveRequest(
  request: SaveAppearancePreferencesRequest,
): SaveAppearancePreferencesRequest {
  return {
    preferences: parseAppearancePreferences(request.preferences),
    expectedRevision: parseAppearanceRevision(request.expectedRevision),
  };
}

function normalizeResetRequest(
  request: ResetAppearancePreferencesRequest,
): ResetAppearancePreferencesRequest {
  return {
    expectedRevision: parseAppearanceRevision(request.expectedRevision),
  };
}

function normalizeImportRequest(
  request: ImportAppearancePreferencesRequest,
): ImportAppearancePreferencesRequest {
  return {
    document: parseThemePackDocument(request.document),
    expectedRevision: parseAppearanceRevision(request.expectedRevision),
  };
}

export async function getAppearanceSettings(): Promise<AppearanceSnapshot> {
  if (!isTauri()) {
    return previewSnapshot();
  }
  return parseAppearanceSnapshot(
    await invoke<unknown>(GET_APPEARANCE_COMMAND),
  );
}

export async function saveAppearancePreferences(
  request: SaveAppearancePreferencesRequest,
): Promise<AppearanceSnapshot> {
  const normalized = normalizeSaveRequest(request);
  if (!isTauri()) {
    throw desktopOnlyError("Saving appearance preferences");
  }
  return parseAppearanceSnapshot(
    await invoke<unknown>(SAVE_APPEARANCE_COMMAND, { request: normalized }),
  );
}

export async function resetAppearancePreferences(
  request: ResetAppearancePreferencesRequest,
): Promise<AppearanceSnapshot> {
  const normalized = normalizeResetRequest(request);
  if (!isTauri()) {
    throw desktopOnlyError("Resetting appearance preferences");
  }
  return parseAppearanceSnapshot(
    await invoke<unknown>(RESET_APPEARANCE_COMMAND, { request: normalized }),
  );
}

export async function importAppearancePreferences(
  request: ImportAppearancePreferencesRequest,
): Promise<AppearanceSnapshot> {
  const normalized = normalizeImportRequest(request);
  if (!isTauri()) {
    throw desktopOnlyError("Importing appearance preferences");
  }
  return parseAppearanceSnapshot(
    await invoke<unknown>(IMPORT_APPEARANCE_COMMAND, { request: normalized }),
  );
}

export async function exportAppearancePreferences(): Promise<ThemePackDocument> {
  if (!isTauri()) {
    return createThemePackDocument(createDefaultAppearancePreferences());
  }
  return parseThemePackDocument(
    await invoke<unknown>(EXPORT_APPEARANCE_COMMAND),
  );
}

export async function subscribeToAppearanceChanges(
  listener: (snapshot: AppearanceSnapshot) => void,
  onError?: (error: Error) => void,
): Promise<() => void> {
  if (!isTauri()) {
    return () => undefined;
  }
  return listen<unknown>(APPEARANCE_CHANGED_EVENT, (event) => {
    try {
      listener(parseAppearanceSnapshot(event.payload));
    } catch (error) {
      onError?.(
        error instanceof Error
          ? error
          : new Error("The appearance event payload was invalid."),
      );
    }
  });
}

export const nativeAppearanceClient: AppearanceClient = {
  getSettings: getAppearanceSettings,
  savePreferences: saveAppearancePreferences,
  resetPreferences: resetAppearancePreferences,
  importPreferences: importAppearancePreferences,
  exportPreferences: exportAppearancePreferences,
  subscribeToChanges: subscribeToAppearanceChanges,
};

export function createAppearanceSnapshot(
  preferences: AppearancePreferences,
  revision = "preview",
): AppearanceSnapshot {
  return {
    preferences: parseAppearancePreferences(preferences),
    revision: parseAppearanceRevision(revision),
    recoveryNotice: null,
  };
}

export function describeAppearanceError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.trim().slice(0, 500);
  }
  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }
  return "The appearance operation could not be completed.";
}
