import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  loadServiceConfiguration,
  parseServiceConfiguration,
} from "../services/config/serviceConfig.js";
import {
  SERVICE_CREDENTIAL_KINDS,
  type ServiceConfigurationSnapshot,
  type ServiceCredentialKind,
  type ServiceCredentialStatus,
  type ServiceSettingsClient,
  type SetServiceCredentialRequest,
} from "./settings.types.js";
import type { ServiceConfiguration } from "../services/service.types";

const GET_CONFIGURATION_COMMAND = "get_service_configuration";
const SAVE_CONFIGURATION_COMMAND = "save_service_configuration";
const RESET_CONFIGURATION_COMMAND = "reset_service_configuration";
const RESTORE_CONFIGURATION_COMMAND = "restore_service_configuration_backup";
const GET_CREDENTIAL_STATUSES_COMMAND = "get_service_credential_statuses";
const SET_CREDENTIAL_COMMAND = "set_service_credential";
const DELETE_CREDENTIAL_COMMAND = "delete_service_credential";
const SETTINGS_STARTUP_RETRY_DELAYS_MS = [25, 50, 100, 200, 400, 800] as const;

const SNAPSHOT_KEYS = new Set([
  "configuration",
  "recoveryNotice",
  "backupAvailable",
]);
const CREDENTIAL_STATUS_KEYS = new Set(["kind", "exists"]);

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

function normalizeRecoveryNotice(value: unknown): string | null {
  if (value === null) {
    return null;
  }

  if (typeof value !== "string") {
    throw new Error("The settings response had an invalid recovery notice.");
  }

  const notice = value.trim();
  if (notice.length === 0 || notice.length > 1_000) {
    throw new Error("The settings response had an invalid recovery notice.");
  }

  return notice;
}

export function parseServiceConfigurationSnapshot(
  value: unknown,
): ServiceConfigurationSnapshot {
  if (!isRecord(value)) {
    throw new Error("The native settings response was not an object.");
  }

  assertExactKeys(value, SNAPSHOT_KEYS, "The native settings response");

  if (typeof value.backupAvailable !== "boolean") {
    throw new Error("The settings response had an invalid backup state.");
  }

  return {
    configuration: parseServiceConfiguration(value.configuration),
    recoveryNotice: normalizeRecoveryNotice(value.recoveryNotice),
    backupAvailable: value.backupAvailable,
  };
}

function parseCredentialKind(value: unknown): ServiceCredentialKind {
  const kind = SERVICE_CREDENTIAL_KINDS.find((candidate) => candidate === value);
  if (kind === undefined) {
    throw new Error("The credential status response had an invalid kind.");
  }

  return kind;
}

function parseCredentialStatusItem(value: unknown): ServiceCredentialStatus {
  if (!isRecord(value)) {
    throw new Error("The credential status response contained an invalid item.");
  }

  assertExactKeys(value, CREDENTIAL_STATUS_KEYS, "A credential status item");
  const kind = parseCredentialKind(value.kind);
  if (typeof value.exists !== "boolean") {
    throw new Error("The credential status response was inconsistent.");
  }

  return { kind, exists: value.exists };
}

export function parseServiceCredentialStatuses(
  value: unknown,
): readonly ServiceCredentialStatus[] {
  if (!Array.isArray(value) || value.length !== SERVICE_CREDENTIAL_KINDS.length) {
    throw new Error("The credential status response was incomplete.");
  }

  const byKind = new Map<ServiceCredentialKind, ServiceCredentialStatus>();

  for (const item of value) {
    const status = parseCredentialStatusItem(item);
    if (byKind.has(status.kind)) {
      throw new Error("The credential status response was inconsistent.");
    }

    byKind.set(status.kind, status);
  }

  return SERVICE_CREDENTIAL_KINDS.map((kind) => {
    const status = byKind.get(kind);
    if (status === undefined) {
      throw new Error("The credential status response was incomplete.");
    }

    return status;
  });
}

function desktopOnlyError(action: string): Error {
  return new Error(`${action} requires the Personal Hub desktop runtime.`);
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }

  return typeof error === "string" ? error : "";
}

export function isTransientSettingsStartupError(error: unknown): boolean {
  const message = errorMessage(error);
  return (
    message.includes("state not managed for field `settings`") &&
    message.includes(GET_CONFIGURATION_COMMAND)
  );
}

type RetryWait = (milliseconds: number) => Promise<void>;

const waitForRetry: RetryWait = (milliseconds) =>
  new Promise((resolve) => {
    globalThis.setTimeout(resolve, milliseconds);
  });

export async function runWithSettingsStartupRetry<T>(
  operation: () => Promise<T>,
  wait: RetryWait = waitForRetry,
): Promise<T> {
  for (const delay of SETTINGS_STARTUP_RETRY_DELAYS_MS) {
    try {
      return await operation();
    } catch (error: unknown) {
      if (!isTransientSettingsStartupError(error)) {
        throw error;
      }

      await wait(delay);
    }
  }

  return operation();
}

export async function getServiceConfiguration(): Promise<ServiceConfigurationSnapshot> {
  if (!isTauri()) {
    return {
      configuration: await loadServiceConfiguration(),
      recoveryNotice: "Preview mode is read-only. Persistence requires the desktop app.",
      backupAvailable: false,
    };
  }

  const response = await runWithSettingsStartupRetry(() =>
    invoke<unknown>(GET_CONFIGURATION_COMMAND),
  );
  return parseServiceConfigurationSnapshot(response);
}

export async function saveServiceConfiguration(
  configuration: ServiceConfiguration,
): Promise<ServiceConfigurationSnapshot> {
  const normalized = parseServiceConfiguration(configuration);
  if (!isTauri()) {
    throw desktopOnlyError("Saving service settings");
  }

  const response = await invoke<unknown>(SAVE_CONFIGURATION_COMMAND, {
    request: { configuration: normalized },
  });
  return parseServiceConfigurationSnapshot(response);
}

export async function resetServiceConfiguration(): Promise<ServiceConfigurationSnapshot> {
  if (!isTauri()) {
    throw desktopOnlyError("Resetting service settings");
  }

  const response = await invoke<unknown>(RESET_CONFIGURATION_COMMAND);
  return parseServiceConfigurationSnapshot(response);
}

export async function restoreServiceConfigurationBackup(): Promise<ServiceConfigurationSnapshot> {
  if (!isTauri()) {
    throw desktopOnlyError("Restoring a service-settings backup");
  }

  const response = await invoke<unknown>(RESTORE_CONFIGURATION_COMMAND);
  return parseServiceConfigurationSnapshot(response);
}

export async function getServiceCredentialStatuses(
  serviceId: string,
): Promise<readonly ServiceCredentialStatus[]> {
  if (!isTauri()) {
    return SERVICE_CREDENTIAL_KINDS.map((kind) => ({ kind, exists: false }));
  }

  const response = await invoke<unknown>(GET_CREDENTIAL_STATUSES_COMMAND, {
    serviceId,
  });
  return parseServiceCredentialStatuses(response);
}

export async function setServiceCredential(
  request: SetServiceCredentialRequest,
): Promise<void> {
  if (!isTauri()) {
    throw desktopOnlyError("Storing credentials");
  }

  const response = await invoke<unknown>(SET_CREDENTIAL_COMMAND, { request });
  const status = parseCredentialStatusItem(response);
  if (status.kind !== request.kind || !status.exists) {
    throw new Error("The credential update response was inconsistent.");
  }
}

export async function deleteServiceCredential(
  serviceId: string,
  kind: ServiceCredentialKind,
): Promise<void> {
  if (!isTauri()) {
    throw desktopOnlyError("Deleting credentials");
  }

  const response = await invoke<unknown>(DELETE_CREDENTIAL_COMMAND, {
    request: { serviceId, kind },
  });
  const status = parseCredentialStatusItem(response);
  if (status.kind !== kind || status.exists) {
    throw new Error("The credential deletion response was inconsistent.");
  }
}

export const nativeServiceSettingsClient: ServiceSettingsClient = {
  getConfiguration: getServiceConfiguration,
  saveConfiguration: saveServiceConfiguration,
  resetConfiguration: resetServiceConfiguration,
  restoreConfigurationBackup: restoreServiceConfigurationBackup,
  getCredentialStatuses: getServiceCredentialStatuses,
  setCredential: setServiceCredential,
  deleteCredential: deleteServiceCredential,
};

export function describeSettingsError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.slice(0, 500);
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }

  return "The settings operation could not be completed.";
}
