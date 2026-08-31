import type {
  DashboardService,
  ServiceConfiguration,
} from "../services/service.types";

export const SERVICE_CREDENTIAL_KINDS = [
  "api-key",
  "bearer-token",
  "http-basic",
  "username-password",
] as const;

export type ServiceCredentialKind =
  (typeof SERVICE_CREDENTIAL_KINDS)[number];

export interface ServiceCredentialStatus {
  kind: ServiceCredentialKind;
  exists: boolean;
}

export interface ServiceConfigurationSnapshot {
  configuration: ServiceConfiguration;
  recoveryNotice: string | null;
  backupAvailable: boolean;
}

export interface SetServiceCredentialRequest {
  serviceId: string;
  kind: ServiceCredentialKind;
  username: string | null;
  secret: string;
}

export interface ServiceSettingsClient {
  getConfiguration: () => Promise<ServiceConfigurationSnapshot>;
  saveConfiguration: (
    configuration: ServiceConfiguration,
  ) => Promise<ServiceConfigurationSnapshot>;
  resetConfiguration: () => Promise<ServiceConfigurationSnapshot>;
  restoreConfigurationBackup: () => Promise<ServiceConfigurationSnapshot>;
  getCredentialStatuses: (
    serviceId: string,
  ) => Promise<readonly ServiceCredentialStatus[]>;
  setCredential: (request: SetServiceCredentialRequest) => Promise<void>;
  deleteCredential: (
    serviceId: string,
    kind: ServiceCredentialKind,
  ) => Promise<void>;
}

export interface SettingsPageProps {
  /** Optional catalog used immediately while the native source of truth loads. */
  initialConfiguration?: ServiceConfiguration;
  /** Opens the editor on this service when Settings was launched from a card. */
  initialServiceId?: string;
  /** Injectable for tests or an alternate host; defaults to the Tauri client. */
  client?: ServiceSettingsClient;
  /** Replaces the dashboard catalog and metadata after native persistence. */
  onConfigurationApplied?: (
    snapshot: ServiceConfigurationSnapshot,
  ) => void | Promise<void>;
  onClose?: () => void;
  className?: string;
}

export type EditableService = DashboardService;
