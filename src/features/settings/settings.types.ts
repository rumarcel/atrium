import type {
  DashboardService,
  ServiceConfiguration,
} from "../services/service.types";
import type { ServiceDiscoveryClient } from "../discovery/discovery.types";

export const SERVICE_CREDENTIAL_KINDS = [
  "api-key",
  "bearer-token",
  "http-basic",
  "username-password",
] as const;

export type ServiceCredentialKind =
  (typeof SERVICE_CREDENTIAL_KINDS)[number];

export const SERVICE_AUTHENTICATION_API_ADAPTERS = [
  "none",
  "homarr-api-key",
  "glances-http-basic",
  "glances-bearer",
] as const;

export type ServiceAuthenticationApiAdapter =
  (typeof SERVICE_AUTHENTICATION_API_ADAPTERS)[number];

export const SERVICE_AUTHENTICATION_BROWSER_ADAPTERS = [
  "none",
  "http-basic",
] as const;

export type ServiceAuthenticationBrowserAdapter =
  (typeof SERVICE_AUTHENTICATION_BROWSER_ADAPTERS)[number];

export const SERVICE_AUTHENTICATION_CREDENTIAL_STATES = [
  "not-required",
  "missing",
  "stored",
  "needs-rebind",
  "vault-unavailable",
] as const;

export type ServiceAuthenticationCredentialState =
  (typeof SERVICE_AUTHENTICATION_CREDENTIAL_STATES)[number];

export const SERVICE_AUTHENTICATION_VALIDATION_STATES = [
  "unsupported",
  "not-validated",
  "validating",
  "valid",
  "invalid",
  "temporarily-unavailable",
  "backoff",
] as const;

export type ServiceAuthenticationValidationState =
  (typeof SERVICE_AUTHENTICATION_VALIDATION_STATES)[number];

export const SERVICE_AUTHENTICATION_REASON_CODES = [
  "missing-credential",
  "endpoint-changed",
  "unauthorized",
  "forbidden",
  "rate-limited",
  "timeout",
  "tls",
  "connection",
  "api-unavailable",
  "invalid-data",
  "insecure-transport",
  "vault-unavailable",
  "validation-in-progress",
] as const;

export type ServiceAuthenticationReasonCode =
  (typeof SERVICE_AUTHENTICATION_REASON_CODES)[number];

export interface ServiceCredentialStatus {
  kind: ServiceCredentialKind;
  exists: boolean;
}

export interface ServiceAuthenticationStatusSnapshot {
  serviceId: string;
  revision: string;
  apiAdapter: ServiceAuthenticationApiAdapter;
  browserAdapter: ServiceAuthenticationBrowserAdapter;
  requiredCredentialKinds: readonly ServiceCredentialKind[];
  credentialState: ServiceAuthenticationCredentialState;
  validationState: ServiceAuthenticationValidationState;
  canValidate: boolean;
  canClearSession: boolean;
  reasonCode: ServiceAuthenticationReasonCode | null;
  retryAfterMs: number | null;
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
  setCredential: (
    request: SetServiceCredentialRequest,
  ) => Promise<ServiceCredentialStatus>;
  deleteCredential: (
    serviceId: string,
    kind: ServiceCredentialKind,
  ) => Promise<ServiceCredentialStatus>;
  getAuthenticationStatus: (
    serviceId: string,
  ) => Promise<ServiceAuthenticationStatusSnapshot>;
  validateAuthentication: (
    serviceId: string,
    expectedRevision: string | null,
  ) => Promise<ServiceAuthenticationStatusSnapshot>;
}

export interface SettingsPageProps {
  /** Optional catalog used immediately while the native source of truth loads. */
  initialConfiguration?: ServiceConfiguration;
  /** Opens the editor on this service when Settings was launched from a card. */
  initialServiceId?: string;
  /** Injectable for tests or an alternate host; defaults to the Tauri client. */
  client?: ServiceSettingsClient;
  /** Injectable read-only discovery transport; defaults to the Tauri client. */
  discoveryClient?: ServiceDiscoveryClient;
  /** Replaces the dashboard catalog and metadata after native persistence. */
  onConfigurationApplied?: (
    snapshot: ServiceConfigurationSnapshot,
  ) => void | Promise<void>;
  onClose?: () => void;
  className?: string;
}

export type EditableService = DashboardService;
