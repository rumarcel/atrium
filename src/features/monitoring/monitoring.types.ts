export type ServerMetricsBackendStatus = "online" | "unavailable";

export interface DiskMetrics {
  name: string;
  mountPoint: string;
  usedBytes: number;
  totalBytes: number;
  percent: number | null;
}

export interface ServerMetricsResult {
  status: ServerMetricsBackendStatus;
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
