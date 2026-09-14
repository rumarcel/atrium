export const SERVER_CONTROL_TARGET = "192.168.1.10:9473" as const;
export const SERVER_CONTROL_HOST = "192.168.1.10" as const;

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
  target: typeof SERVER_CONTROL_TARGET;
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
