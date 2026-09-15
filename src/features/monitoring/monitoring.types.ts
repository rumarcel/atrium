export type ServerMetricsBackendStatus = "online" | "unavailable";
export type MonitoringProviderState = "configured" | "not-configured";
export type MonitoringUnavailableReason =
  | "authentication"
  | "timeout"
  | "tls"
  | "connection"
  | "api-unavailable"
  | "invalid-data"
  | "not-configured";

export interface DiskMetrics {
  name: string;
  fileSystem: string | null;
  mountPoint: string;
  usedBytes: number;
  totalBytes: number;
  percent: number | null;
}

export interface ServerMetricsResult {
  status: ServerMetricsBackendStatus;
  providerState: MonitoringProviderState;
  reason: MonitoringUnavailableReason | null;
  sampledAt: number;
  cpuPercent: number | null;
  memoryPercent: number | null;
  memoryUsedBytes: number | null;
  memoryTotalBytes: number | null;
  cpuTemperatureC: number | null;
  networkDownloadBytesPerSecond: number | null;
  networkUploadBytesPerSecond: number | null;
  uptimeSeconds: number | null;
  loadAverage1m: number | null;
  loadAverage5m: number | null;
  loadAverage15m: number | null;
  disks: readonly DiskMetrics[];
  message: string | null;
}

export interface ServerMetricsTrendSample {
  sampledAt: number;
  cpuPercent: number | null;
  memoryPercent: number | null;
  networkDownloadBytesPerSecond: number | null;
  networkUploadBytesPerSecond: number | null;
  loadAverage1m: number | null;
}

export type ServerMonitoringStatus =
  | "loading"
  | ServerMetricsBackendStatus;

export interface ServerMetricsMonitor {
  status: ServerMonitoringStatus;
  providerState: MonitoringProviderState;
  unavailableReason: MonitoringUnavailableReason | null;
  /** The newest successful sample. It remains available during a brief outage. */
  snapshot: ServerMetricsResult | null;
  /** A bounded in-memory window; it is never persisted or rendered as timestamps. */
  history: readonly ServerMetricsTrendSample[];
  isRefreshing: boolean;
  isPaused: boolean;
  isStale: boolean;
  message: string | null;
  lastUpdatedAt: number | null;
  refresh: () => void;
}

export interface UseServerMetricsOptions {
  enabled?: boolean;
  pollIntervalMs?: number;
  staleAfterMs?: number;
}
