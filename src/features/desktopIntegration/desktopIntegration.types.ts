export interface DesktopPreferences {
  version: 1;
  notifyServiceOutages: boolean;
  notifyStoragePressure: boolean;
  notifyDownloadCompletion: boolean;
}

export interface DesktopIntegrationSnapshot {
  preferences: DesktopPreferences;
  revision: string;
  startupEnabled: boolean;
  startupSupported: boolean;
  notificationsSupported: boolean;
  appVersion: string;
  buildProfile: "debug" | "release";
  platform: string;
  architecture: string;
  signingStatus: "not-verified";
  recoveryNotice: string | null;
}

export interface SaveDesktopIntegrationSettingsRequest {
  preferences: DesktopPreferences;
  expectedRevision: string;
}

export interface DesktopIntegrationClient {
  getSettings: () => Promise<DesktopIntegrationSnapshot>;
  saveSettings: (request: SaveDesktopIntegrationSettingsRequest) => Promise<DesktopIntegrationSnapshot>;
  setStartupEnabled: (enabled: boolean) => Promise<DesktopIntegrationSnapshot>;
}
