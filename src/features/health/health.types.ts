export type BackendHealthStatus = "online" | "offline" | "warning";

export type ServiceHealthStatus = BackendHealthStatus | "unchecked";

export type ServiceHealthReason =
  | "timeout"
  | "connection"
  | "tls"
  | "tls-exception"
  | "http-status"
  | "invalid-request"
  | "request"
  | "runtime"
  | "invoke";

export interface HealthCheckResult {
  serviceId: string;
  status: BackendHealthStatus;
  statusCode: number | null;
  latencyMs: number;
  checkedAtUnixMs: number;
  reason: ServiceHealthReason | null;
  message: string | null;
  tlsExceptionUsed: boolean;
}

export interface ServiceHealth extends Omit<HealthCheckResult, "status"> {
  status: ServiceHealthStatus;
  isChecking: boolean;
}

export type ServiceHealthById = Readonly<
  Record<string, ServiceHealth | undefined>
>;

export interface ServiceHealthSummary {
  online: number;
  offline: number;
  warning: number;
  unchecked: number;
  total: number;
}
