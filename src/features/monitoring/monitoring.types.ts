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
  disks: readonly DiskMetrics[];
  message: string | null;
}

export type ServerMonitoringStatus =
  | "loading"
  | ServerMetricsBackendStatus;

export interface ServerMetricsMonitor {
  status: ServerMonitoringStatus;
  /** The newest successful sample. It remains available during a brief outage. */
  snapshot: ServerMetricsResult | null;
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
