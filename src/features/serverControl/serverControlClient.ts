import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  isPrivateIpv4,
  SERVER_CONNECTION_STATES,
  SERVER_CONTROL_PORT,
  SERVER_OPERATION_STATES,
  type ServerControlAction,
  type ServerControlClient,
  type ServerControlOperation,
  type ServerControlSnapshot,
} from "./serverControl.types.js";

function record(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("Invalid server control object.");
  const result = value as Record<string, unknown>;
  if (Object.keys(result).length !== keys.length || keys.some((key) => !Object.prototype.hasOwnProperty.call(result, key))) {
    throw new Error("Unexpected server control fields.");
  }
  return result;
}

function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") throw new Error("Invalid server control flag.");
  return value;
}

function text(value: unknown, maximum = 128, allowEmpty = false): string {
  if (typeof value !== "string" || (value.length === 0 && !allowEmpty) || value.length > maximum || value.trim() !== value || /[\u0000-\u001f\u007f]/.test(value)) {
    throw new Error("Invalid server control text.");
  }
  return value;
}

function integer(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw new Error("Invalid server control number.");
  return value;
}

function revision(value: unknown): string {
  const result = text(value, 20);
  if (!/^(0|[1-9][0-9]*)$/.test(result) || (result.length === 20 && result > "18446744073709551615")) throw new Error("Invalid server control revision.");
  return result;
}

function certificate(value: unknown): string {
  if (typeof value !== "string" || value.length > 16_384 || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(value)) {
    throw new Error("Invalid server certificate.");
  }
  return value;
}

function member<const T extends readonly string[]>(value: unknown, allowed: T): T[number] {
  if (typeof value !== "string" || !allowed.includes(value)) throw new Error("Invalid server control state.");
  return value as T[number];
}

function action(value: unknown): ServerControlAction {
  return member(value, ["reboot", "shutdown"] as const);
}

function operation(value: unknown): ServerControlOperation {
  const data = record(value, ["id", "action", "state", "requestedAt", "executeAt", "observedOffline", "dryRun"]);
  return {
    id: text(data.id), action: action(data.action), state: member(data.state, SERVER_OPERATION_STATES),
    requestedAt: integer(data.requestedAt), executeAt: data.executeAt === null ? null : integer(data.executeAt),
    observedOffline: boolean(data.observedOffline), dryRun: boolean(data.dryRun),
  };
}

export function parseServerControlSnapshot(value: unknown): ServerControlSnapshot {
  const data = record(value, ["target", "address", "enabled", "certificatePem", "credentialStored", "revision", "connectionState", "serverStatus", "pendingConfirmation", "activeOperation", "history", "notice"]);
  const address = text(data.address, 45, true);
  // Either nothing is enrolled, or the target is exactly that address on the
  // agent's fixed port. A target naming any other host or port is rejected.
  if (address === "") {
    if (data.target !== "") throw new Error("Unexpected server control target.");
  } else if (!isPrivateIpv4(address) || data.target !== `${address}:${SERVER_CONTROL_PORT}`) {
    throw new Error("Unexpected server control target.");
  }
  let serverStatus: ServerControlSnapshot["serverStatus"] = null;
  if (data.serverStatus !== null) {
    const status = record(data.serverStatus, ["bootId", "uptimeSeconds", "dryRun"]);
    serverStatus = { bootId: text(status.bootId), uptimeSeconds: integer(status.uptimeSeconds), dryRun: boolean(status.dryRun) };
  }
  let pendingConfirmation: ServerControlSnapshot["pendingConfirmation"] = null;
  if (data.pendingConfirmation !== null) {
    const pending = record(data.pendingConfirmation, ["id", "action", "expiresAt", "dryRun"]);
    pendingConfirmation = { id: text(pending.id), action: action(pending.action), expiresAt: integer(pending.expiresAt), dryRun: boolean(pending.dryRun) };
  }
  if (!Array.isArray(data.history) || data.history.length > 100) throw new Error("Invalid server operation history.");
  const history = data.history.map(operation);
  if (new Set(history.map((entry) => entry.id)).size !== history.length) throw new Error("Duplicate server operation history.");
  const snapshot: ServerControlSnapshot = {
    target: text(data.target, 64, true), address, enabled: boolean(data.enabled), certificatePem: certificate(data.certificatePem),
    credentialStored: boolean(data.credentialStored), revision: revision(data.revision),
    connectionState: member(data.connectionState, SERVER_CONNECTION_STATES), serverStatus, pendingConfirmation,
    activeOperation: data.activeOperation === null ? null : operation(data.activeOperation), history,
    notice: data.notice === null ? null : text(data.notice, 1_000),
  };
  if (snapshot.connectionState === "online" && (!snapshot.enabled || !snapshot.address || !snapshot.credentialStored || !snapshot.certificatePem.trim() || serverStatus === null)) {
    throw new Error("Inconsistent server connection status.");
  }
  if (snapshot.pendingConfirmation !== null && snapshot.activeOperation !== null) throw new Error("Conflicting server operations.");
  return snapshot;
}

// Serialize across component remounts too. Never retry a power mutation after
// an ambiguous response; only an explicit read may recover its status.
let requestQueue: Promise<unknown> = Promise.resolve();
function request(command: string, args?: Record<string, unknown>): Promise<ServerControlSnapshot> {
  const next = requestQueue.then(async () => {
    if (!isTauri()) throw new Error("Server control requires the desktop app.");
    return parseServerControlSnapshot(await invoke<unknown>(command, args));
  });
  requestQueue = next.then(() => undefined, () => undefined);
  return next;
}

export const nativeServerControlClient: ServerControlClient = {
  getSnapshot: () => request("get_server_control_snapshot"),
  refreshStatus: () => request("refresh_server_control_status"),
  saveSettings(input) {
    if (input.token !== null && !/^[a-fA-F0-9]{64}$/.test(input.token)) throw new Error("Invalid server agent token.");
    if (input.clearToken && input.token !== null) throw new Error("Conflicting server token update.");
    if (input.address !== "" && !isPrivateIpv4(input.address)) throw new Error("Invalid server address.");
    if (input.enabled && input.address === "") throw new Error("A server address is required.");
    return request("save_server_control_settings", { request: {
      enabled: boolean(input.enabled), address: input.address, certificatePem: certificate(input.certificatePem), token: input.token,
      clearToken: boolean(input.clearToken), expectedRevision: revision(input.expectedRevision),
    } });
  },
  prepareAction: (value) => request("prepare_server_control_action", { request: { action: action(value) } }),
  confirmAction(confirmationId, target) {
    // Rust compares this against the enrolled address; this only stops an
    // obviously malformed value from reaching a privileged command.
    if (!isPrivateIpv4(target)) throw new Error("Server target confirmation does not match.");
    return request("confirm_server_control_action", { request: { confirmationId: text(confirmationId), target } });
  },
  cancelOperation: (operationId) => request("cancel_server_control_operation", { request: { operationId: text(operationId) } }),
  dismissConfirmation: () => request("dismiss_server_control_confirmation"),
};
