import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  DesktopIntegrationClient,
  DesktopIntegrationSnapshot,
  DesktopPreferences,
} from "./desktopIntegration.types";

export const DEFAULT_DESKTOP_PREFERENCES: DesktopPreferences = {
  version: 1,
  notifyServiceOutages: false,
  notifyStoragePressure: false,
  notifyDownloadCompletion: false,
};

function record(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("The desktop settings response must be an object.");
  }
  const result = value as Record<string, unknown>;
  if (Object.keys(result).length !== keys.length || keys.some((key) => !Object.prototype.hasOwnProperty.call(result, key))) {
    throw new Error("The desktop settings response has unexpected fields.");
  }
  return result;
}

function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") throw new Error("Invalid desktop settings flag.");
  return value;
}

function text(value: unknown, maximum = 128): string {
  if (typeof value !== "string" || value.length === 0 || value.length > maximum || value.trim() !== value || /[\u0000-\u001f\u007f]/.test(value)) {
    throw new Error("Invalid desktop settings text.");
  }
  return value;
}

function revision(value: unknown): string {
  const result = text(value, 20);
  if (!/^(0|[1-9][0-9]*)$/.test(result) || (result.length === 20 && result > "18446744073709551615")) {
    throw new Error("Invalid desktop settings revision.");
  }
  return result;
}

export function parseDesktopPreferences(value: unknown): DesktopPreferences {
  const data = record(value, ["version", "notifyServiceOutages", "notifyStoragePressure", "notifyDownloadCompletion"]);
  if (data.version !== 1) throw new Error("Unsupported desktop preferences version.");
  return {
    version: 1,
    notifyServiceOutages: boolean(data.notifyServiceOutages),
    notifyStoragePressure: boolean(data.notifyStoragePressure),
    notifyDownloadCompletion: boolean(data.notifyDownloadCompletion),
  };
}

export function parseDesktopIntegrationSnapshot(value: unknown): DesktopIntegrationSnapshot {
  const data = record(value, [
    "preferences", "revision", "startupEnabled", "startupSupported", "notificationsSupported",
    "appVersion", "buildProfile", "platform", "architecture", "signingStatus", "recoveryNotice",
  ]);
  if (data.buildProfile !== "debug" && data.buildProfile !== "release") {
    throw new Error("Invalid desktop build profile.");
  }
  if (data.signingStatus !== "not-verified") throw new Error("Invalid signing verification status.");
  const snapshot: DesktopIntegrationSnapshot = {
    preferences: parseDesktopPreferences(data.preferences),
    revision: revision(data.revision),
    startupEnabled: boolean(data.startupEnabled),
    startupSupported: boolean(data.startupSupported),
    notificationsSupported: boolean(data.notificationsSupported),
    appVersion: text(data.appVersion),
    buildProfile: data.buildProfile,
    platform: text(data.platform, 32),
    architecture: text(data.architecture, 32),
    signingStatus: data.signingStatus,
    recoveryNotice: data.recoveryNotice === null ? null : text(data.recoveryNotice, 1_000),
  };
  if ((snapshot.startupEnabled && !snapshot.startupSupported) ||
      (snapshot.startupSupported && (snapshot.platform !== "windows" || snapshot.buildProfile !== "release")) ||
      (snapshot.notificationsSupported && snapshot.platform !== "windows")) {
    throw new Error("Inconsistent desktop integration support.");
  }
  return snapshot;
}

function requireDesktop(): void {
  if (!isTauri()) throw new Error("Desktop integration requires the Personal Hub desktop app.");
}

export const nativeDesktopIntegrationClient: DesktopIntegrationClient = {
  async getSettings() {
    requireDesktop();
    return parseDesktopIntegrationSnapshot(await invoke<unknown>("get_desktop_integration_settings"));
  },
  async saveSettings(request) {
    requireDesktop();
    return parseDesktopIntegrationSnapshot(await invoke<unknown>("save_desktop_integration_settings", {
      request: {
        preferences: parseDesktopPreferences(request.preferences),
        expectedRevision: revision(request.expectedRevision),
      },
    }));
  },
  async setStartupEnabled(enabled) {
    requireDesktop();
    return parseDesktopIntegrationSnapshot(await invoke<unknown>("set_desktop_startup_enabled", { enabled: boolean(enabled) }));
  },
};
