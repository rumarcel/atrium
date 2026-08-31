export const DESKTOP_WIDGET_KINDS = [
  "server",
  "storage",
  "services",
] as const;

export type DesktopWidgetKind = (typeof DESKTOP_WIDGET_KINDS)[number];

export const DESKTOP_WIDGET_HASHES: Readonly<
  Record<DesktopWidgetKind, string>
> = {
  server: "#desktop-widget/server",
  storage: "#desktop-widget/storage",
  services: "#desktop-widget/services",
};

export type DesktopWidgetSnapshotStatus = "online" | "unavailable";
export type DesktopWidgetServiceStatus = "online" | "offline" | "warning";
export type DesktopWidgetProviderState = "configured" | "not-configured";
export type DesktopWidgetUnavailableReason =
  | "authentication"
  | "timeout"
  | "tls"
  | "connection"
  | "api-unavailable"
  | "invalid-data"
  | "not-configured";

export interface DesktopWidgetDisk {
  name: string;
  mountPoint: string;
  usedBytes: number;
  totalBytes: number;
  percent: number | null;
}

export interface DesktopWidgetMetrics {
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
  disks: readonly DesktopWidgetDisk[];
}

export interface DesktopWidgetService {
  id: string;
  name: string;
  status: DesktopWidgetServiceStatus;
  message: string | null;
}

export interface DesktopWidgetTrendSample {
  sampledAt: number;
  cpuPercent: number | null;
  memoryPercent: number | null;
  networkDownloadBytesPerSecond: number | null;
  networkUploadBytesPerSecond: number | null;
  loadAverage1m: number | null;
}

export interface DesktopWidgetSnapshot {
  status: DesktopWidgetSnapshotStatus;
  providerState: DesktopWidgetProviderState;
  reason: DesktopWidgetUnavailableReason | null;
  sampledAt: number;
  serverName: string;
  serverAddress: string;
  metrics: DesktopWidgetMetrics;
  services: readonly DesktopWidgetService[];
  trends: readonly DesktopWidgetTrendSample[];
  message: string | null;
}

export interface DesktopWidgetMonitor {
  snapshot: DesktopWidgetSnapshot | null;
  isLoading: boolean;
  isRefreshing: boolean;
  isPaused: boolean;
  isStale: boolean;
  error: string | null;
  refresh: () => void;
}

export function desktopWidgetKindFromHash(
  hash: string,
): DesktopWidgetKind | null {
  const match = DESKTOP_WIDGET_KINDS.find(
    (kind) => DESKTOP_WIDGET_HASHES[kind] === hash,
  );

  return match ?? null;
}
