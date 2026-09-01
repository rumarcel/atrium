import type { DesktopWidgetKind } from "../desktopWidgets/desktopWidget.types";

export const BACKGROUND_RUNTIME_AVAILABILITIES = [
  "available",
  "glances-not-configured",
  "no-health-targets",
] as const;

export type BackgroundRuntimeAvailability =
  (typeof BACKGROUND_RUNTIME_AVAILABILITIES)[number];

export type DesktopCardPreferences = Record<DesktopWidgetKind, boolean>;

export interface BackgroundRuntimePreferences {
  version: 1;
  experimentalDesktopCards: boolean;
  closeToTray: boolean;
  cards: DesktopCardPreferences;
}

export type DesktopCardAvailability = Record<
  DesktopWidgetKind,
  BackgroundRuntimeAvailability
>;

export interface BackgroundRuntimeSnapshot {
  preferences: BackgroundRuntimePreferences;
  revision: string;
  availability: DesktopCardAvailability;
  trayAvailable: boolean;
  recoveryNotice: string | null;
}

export interface SaveBackgroundRuntimePreferencesRequest {
  preferences: BackgroundRuntimePreferences;
  expectedRevision: string;
}

export interface BackgroundRuntimeClient {
  getPreferences: () => Promise<BackgroundRuntimeSnapshot>;
  savePreferences: (
    request: SaveBackgroundRuntimePreferencesRequest,
  ) => Promise<BackgroundRuntimeSnapshot>;
}
