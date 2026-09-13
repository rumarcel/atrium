import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  loadServiceConfiguration,
  parseServiceConfiguration,
} from "../services/config/serviceConfig.js";
import {
  SERVICE_AUTHENTICATION_API_ADAPTERS,
  SERVICE_AUTHENTICATION_BROWSER_ADAPTERS,
  SERVICE_AUTHENTICATION_CREDENTIAL_STATES,
  SERVICE_AUTHENTICATION_REASON_CODES,
  SERVICE_AUTHENTICATION_VALIDATION_STATES,
  SERVICE_CREDENTIAL_KINDS,
  type ServiceAuthenticationStatusSnapshot,
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
const GET_AUTHENTICATION_STATUS_COMMAND = "get_service_authentication_status";
const VALIDATE_AUTHENTICATION_COMMAND = "validate_service_authentication";
const SETTINGS_STARTUP_RETRY_DELAYS_MS = [25, 50, 100, 200, 400, 800] as const;

const SNAPSHOT_KEYS = new Set([
  "configuration",
  "recoveryNotice",
  "backupAvailable",
]);
const CREDENTIAL_STATUS_KEYS = new Set(["kind", "exists"]);
const AUTHENTICATION_STATUS_KEYS = new Set([
  "serviceId",
  "revision",
  "apiAdapter",
  "browserAdapter",
  "requiredCredentialKinds",
  "credentialState",
  "validationState",
  "canValidate",
  "canClearSession",
  "reasonCode",
  "retryAfterMs",
]);

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

function parseEnum<const Values extends readonly string[]>(
  value: unknown,
  values: Values,
  context: string,
): Values[number] {
  const parsed = values.find((candidate) => candidate === value);
  if (parsed === undefined) {
    throw new Error(`${context} had an invalid value.`);
  }

  return parsed;
}

function parseAuthenticationString(
  value: unknown,
  field: "service ID" | "revision",
  maximumLength: number,
): string {
  if (typeof value !== "string") {
    throw new Error(`The authentication response had an invalid ${field}.`);
  }

  const parsed = value.trim();
  if (parsed.length === 0 || parsed.length > maximumLength || parsed !== value) {
    throw new Error(`The authentication response had an invalid ${field}.`);
  }

  return parsed;
}

function parseRequiredCredentialKinds(
  value: unknown,
): readonly ServiceCredentialKind[] {
  if (!Array.isArray(value) || value.length > SERVICE_CREDENTIAL_KINDS.length) {
    throw new Error(
      "The authentication response had invalid required credential kinds.",
    );
  }

  const parsed = value.map(parseCredentialKind);
  if (new Set(parsed).size !== parsed.length) {
    throw new Error(
      "The authentication response had invalid required credential kinds.",
    );
  }

  return parsed;
}

export function parseServiceAuthenticationStatus(
  value: unknown,
): ServiceAuthenticationStatusSnapshot {
  if (!isRecord(value)) {
    throw new Error("The native authentication response was not an object.");
  }

  assertExactKeys(
    value,
    AUTHENTICATION_STATUS_KEYS,
    "The native authentication response",
  );

  if (typeof value.canValidate !== "boolean") {
    throw new Error("The authentication response had invalid validation capability.");
  }
  if (typeof value.canClearSession !== "boolean") {
    throw new Error("The authentication response had invalid session capability.");
  }
  if (
    value.retryAfterMs !== null &&
    (typeof value.retryAfterMs !== "number" ||
      !Number.isSafeInteger(value.retryAfterMs) ||
      value.retryAfterMs < 0)
  ) {
    throw new Error("The authentication response had an invalid retry delay.");
  }

  const snapshot: ServiceAuthenticationStatusSnapshot = {
    serviceId: parseAuthenticationString(value.serviceId, "service ID", 64),
    revision: parseAuthenticationString(value.revision, "revision", 512),
    apiAdapter: parseEnum(
      value.apiAdapter,
      SERVICE_AUTHENTICATION_API_ADAPTERS,
      "The authentication API adapter",
    ),
    browserAdapter: parseEnum(
      value.browserAdapter,
      SERVICE_AUTHENTICATION_BROWSER_ADAPTERS,
      "The authentication browser adapter",
    ),
    requiredCredentialKinds: parseRequiredCredentialKinds(
      value.requiredCredentialKinds,
    ),
    credentialState: parseEnum(
      value.credentialState,
      SERVICE_AUTHENTICATION_CREDENTIAL_STATES,
      "The authentication credential state",
    ),
    validationState: parseEnum(
      value.validationState,
      SERVICE_AUTHENTICATION_VALIDATION_STATES,
      "The authentication validation state",
    ),
    canValidate: value.canValidate,
    canClearSession: value.canClearSession,
    reasonCode:
      value.reasonCode === null
        ? null
        : parseEnum(
            value.reasonCode,
            SERVICE_AUTHENTICATION_REASON_CODES,
            "The authentication reason code",
          ),
    retryAfterMs: value.retryAfterMs,
  };

  assertServiceAuthenticationStatusInvariants(snapshot);
  return snapshot;
}

function expectedAuthenticationCredentialKinds(
  snapshot: ServiceAuthenticationStatusSnapshot,
): readonly ServiceCredentialKind[] {
  const required = new Set<ServiceCredentialKind>();
  switch (snapshot.apiAdapter) {
    case "none":
      break;
    case "homarr-api-key":
      required.add("api-key");
      break;
    case "glances-http-basic":
      required.add("http-basic");
      break;
    case "glances-bearer":
      required.add("bearer-token");
      break;
    case "qbittorrent-web-api":
      required.add("username-password");
      break;
  }
  if (snapshot.browserAdapter === "http-basic") {
    required.add("http-basic");
  }

  return SERVICE_CREDENTIAL_KINDS.filter((kind) => required.has(kind));
}

function authenticationStatusIsInconsistent(message: string): never {
  throw new Error(`The authentication response was inconsistent: ${message}.`);
}

export function assertServiceAuthenticationStatusInvariants(
  snapshot: ServiceAuthenticationStatusSnapshot,
): void {
  if (
    snapshot.apiAdapter === "glances-bearer" &&
    snapshot.browserAdapter === "http-basic"
  ) {
    authenticationStatusIsInconsistent(
      "Glances bearer and browser HTTP Basic cannot share the Authorization header",
    );
  }

  const expectedKinds = expectedAuthenticationCredentialKinds(snapshot);
  if (
    expectedKinds.length !== snapshot.requiredCredentialKinds.length ||
    expectedKinds.some(
      (kind, index) => snapshot.requiredCredentialKinds[index] !== kind,
    )
  ) {
    authenticationStatusIsInconsistent(
      "required credential kinds did not match the selected adapters",
    );
  }

  const hasAdapter =
    snapshot.apiAdapter !== "none" || snapshot.browserAdapter !== "none";
  if (!hasAdapter) {
    if (
      snapshot.credentialState !== "not-required" ||
      snapshot.validationState !== "unsupported" ||
      snapshot.canValidate
    ) {
      authenticationStatusIsInconsistent(
        "services without adapters must report authentication as unsupported",
      );
    }
  } else if (snapshot.credentialState === "not-required") {
    authenticationStatusIsInconsistent(
      "configured adapters must require their mapped credential kinds",
    );
  }

  if (
    snapshot.validationState === "valid" &&
    (!hasAdapter || snapshot.credentialState !== "stored")
  ) {
    authenticationStatusIsInconsistent(
      "valid authentication requires a configured adapter and stored credentials",
    );
  }

  if (
    ["missing", "needs-rebind", "vault-unavailable"].includes(
      snapshot.credentialState,
    ) &&
    (snapshot.canValidate || snapshot.validationState === "valid")
  ) {
    authenticationStatusIsInconsistent(
      "unavailable credentials cannot be validated",
    );
  }

  if (snapshot.validationState === "backoff") {
    if (snapshot.retryAfterMs === null || snapshot.retryAfterMs <= 0) {
      authenticationStatusIsInconsistent(
        "backoff must include a positive retry delay",
      );
    }
  } else if (snapshot.retryAfterMs !== null) {
    authenticationStatusIsInconsistent(
      "retry delay is only valid while authentication is in backoff",
    );
  }

  const reason = snapshot.reasonCode;
  const validation = snapshot.validationState;
  const credential = snapshot.credentialState;
  const compatibleReason = (() => {
    switch (reason) {
      case null:
        return ["unsupported", "not-validated", "valid"].includes(validation);
      case "missing-credential":
        return credential === "missing" && validation === "not-validated";
      case "endpoint-changed":
        return credential === "needs-rebind" && validation === "not-validated";
      case "vault-unavailable":
        return (
          credential === "vault-unavailable" &&
          ["not-validated", "temporarily-unavailable"].includes(validation)
        );
      case "insecure-transport":
        return credential === "stored" && validation === "temporarily-unavailable";
      case "unauthorized":
      case "forbidden":
        return (
          credential === "stored" &&
          (validation === "invalid" || validation === "backoff")
        );
      case "rate-limited":
        return credential === "stored" && validation === "backoff";
      case "validation-in-progress":
        return credential === "stored" && validation === "validating";
      case "timeout":
      case "tls":
      case "connection":
      case "api-unavailable":
      case "invalid-data":
        return credential === "stored" && validation === "temporarily-unavailable";
    }
  })();

  if (!compatibleReason) {
    authenticationStatusIsInconsistent(
      "reason code did not match the credential and validation states",
    );
  }
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
): Promise<ServiceCredentialStatus> {
  if (!isTauri()) {
    throw desktopOnlyError("Storing credentials");
  }

  const response = await invoke<unknown>(SET_CREDENTIAL_COMMAND, { request });
  const status = parseCredentialStatusItem(response);
  if (status.kind !== request.kind || !status.exists) {
    throw new Error("The credential update response was inconsistent.");
  }

  return status;
}

export async function deleteServiceCredential(
  serviceId: string,
  kind: ServiceCredentialKind,
): Promise<ServiceCredentialStatus> {
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

  return status;
}

function assertAuthenticationServiceId(
  snapshot: ServiceAuthenticationStatusSnapshot,
  serviceId: string,
): ServiceAuthenticationStatusSnapshot {
  if (snapshot.serviceId !== serviceId) {
    throw new Error("The authentication response was for a different service.");
  }

  return snapshot;
}

export async function getServiceAuthenticationStatus(
  serviceId: string,
): Promise<ServiceAuthenticationStatusSnapshot> {
  if (!isTauri()) {
    return {
      serviceId,
      revision: "preview",
      apiAdapter: "none",
      browserAdapter: "none",
      requiredCredentialKinds: [],
      credentialState: "not-required",
      validationState: "unsupported",
      canValidate: false,
      canClearSession: false,
      reasonCode: null,
      retryAfterMs: null,
    };
  }

  const response = await invoke<unknown>(GET_AUTHENTICATION_STATUS_COMMAND, {
    serviceId,
  });
  return assertAuthenticationServiceId(
    parseServiceAuthenticationStatus(response),
    serviceId,
  );
}

export async function validateServiceAuthentication(
  serviceId: string,
  expectedRevision: string | null,
): Promise<ServiceAuthenticationStatusSnapshot> {
  if (!isTauri()) {
    throw desktopOnlyError("Validating automatic authentication");
  }

  const response = await invoke<unknown>(VALIDATE_AUTHENTICATION_COMMAND, {
    request: { serviceId, expectedRevision },
  });
  return assertAuthenticationServiceId(
    parseServiceAuthenticationStatus(response),
    serviceId,
  );
}

export const nativeServiceSettingsClient: ServiceSettingsClient = {
  getConfiguration: getServiceConfiguration,
  saveConfiguration: saveServiceConfiguration,
  resetConfiguration: resetServiceConfiguration,
  restoreConfigurationBackup: restoreServiceConfigurationBackup,
  getCredentialStatuses: getServiceCredentialStatuses,
  setCredential: setServiceCredential,
  deleteCredential: deleteServiceCredential,
  getAuthenticationStatus: getServiceAuthenticationStatus,
  validateAuthentication: validateServiceAuthentication,
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
