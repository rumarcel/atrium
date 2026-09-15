/// The companion agent binds one fixed port; only the host is configurable.
export const SERVER_CONTROL_PORT = 9473 as const;

/// Mirrors the native rule: a bare private, loopback or link-local IPv4
/// literal. The desktop still treats Rust as the authority; this only keeps an
/// obviously wrong value from reaching a privileged command.
export function isPrivateIpv4(value: string): boolean {
  const parts = value.split(".");
  if (parts.length !== 4) return false;
  const octets = parts.map((part) => (/^(0|[1-9]\d{0,2})$/.test(part) ? Number(part) : -1));
  if (octets.some((octet) => octet < 0 || octet > 255)) return false;
  const [first, second] = octets;
  return (
    first === 10 ||
    first === 127 ||
    (first === 192 && second === 168) ||
    (first === 172 && second >= 16 && second <= 31) ||
    (first === 169 && second === 254)
  );
}

export const SERVER_CONNECTION_STATES = [
  "not-configured", "disabled", "unchecked", "online", "unreachable", "unauthorized", "invalid-response",
] as const;
export const SERVER_OPERATION_STATES = [
  "dispatching", "uncertain", "scheduled", "executing", "cancelled", "failed", "interrupted",
  "completed", "awaiting-return", "timed-out",
] as const;

export type ServerControlAction = "reboot" | "shutdown";
export type ServerConnectionState = typeof SERVER_CONNECTION_STATES[number];
export type ServerOperationState = typeof SERVER_OPERATION_STATES[number];

export interface ServerControlOperation {
  id: string;
  action: ServerControlAction;
  state: ServerOperationState;
  requestedAt: number;
  executeAt: number | null;
  observedOffline: boolean;
  dryRun: boolean;
}

export interface ServerControlSnapshot {
  /// `host:port` of the enrolled agent, or empty when no address is saved.
  target: string;
  address: string;
  enabled: boolean;
  certificatePem: string;
  credentialStored: boolean;
  revision: string;
  connectionState: ServerConnectionState;
  serverStatus: null | { bootId: string; uptimeSeconds: number; dryRun: boolean };
  pendingConfirmation: null | { id: string; action: ServerControlAction; expiresAt: number; dryRun: boolean };
  activeOperation: ServerControlOperation | null;
  history: ServerControlOperation[];
  notice: string | null;
}

export interface SaveServerControlSettingsRequest {
  enabled: boolean;
  address: string;
  certificatePem: string;
  token: string | null;
  clearToken: boolean;
  expectedRevision: string;
}

export interface ServerControlClient {
  getSnapshot: () => Promise<ServerControlSnapshot>;
  refreshStatus: () => Promise<ServerControlSnapshot>;
  saveSettings: (request: SaveServerControlSettingsRequest) => Promise<ServerControlSnapshot>;
  prepareAction: (action: ServerControlAction) => Promise<ServerControlSnapshot>;
  confirmAction: (confirmationId: string, target: string) => Promise<ServerControlSnapshot>;
  cancelOperation: (operationId: string) => Promise<ServerControlSnapshot>;
  dismissConfirmation: () => Promise<ServerControlSnapshot>;
}
