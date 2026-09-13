export const DOWNLOAD_ITEM_STATES = [
  "downloading",
  "queued",
  "stalled",
  "paused",
  "checking",
  "metadata",
  "error",
  "other",
] as const;

export type DownloadItemState = (typeof DOWNLOAD_ITEM_STATES)[number];

export type DownloadCenterBackendStatus = "online" | "unavailable";
export type DownloadCenterProviderState = "configured" | "not-configured";
export type DownloadCenterUnavailableReason =
  | "authentication"
  | "timeout"
  | "tls"
  | "connection"
  | "api-unavailable"
  | "invalid-data"
  | "backoff";

export interface DownloadProvider {
  serviceId: string;
  name: string;
}

export interface DownloadSource {
  kind: "sonarr" | "radarr";
  serviceId: string;
  label: string;
}

export interface DownloadItem {
  id: string;
  name: string;
  progressPercent: number;
  downloadSpeedBytesPerSecond: number;
  etaSeconds: number | null;
  state: DownloadItemState;
  category: string | null;
  tags: readonly string[];
  source: DownloadSource | null;
}

export interface DownloadCenterResult {
  status: DownloadCenterBackendStatus;
  providerState: DownloadCenterProviderState;
  reason: DownloadCenterUnavailableReason | null;
  sampledAt: number;
  retryAfterMs: number | null;
  provider: DownloadProvider | null;
  totalDownloadSpeedBytesPerSecond: number;
  items: readonly DownloadItem[];
  message: string | null;
}

export interface DownloadCenterMonitor {
  status: "loading" | DownloadCenterBackendStatus;
  providerState: DownloadCenterProviderState;
  reason: DownloadCenterUnavailableReason | null;
  result: DownloadCenterResult | null;
  /** The newest successful result remains available during a brief outage. */
  snapshot: DownloadCenterResult | null;
  isRefreshing: boolean;
  isPaused: boolean;
  isStale: boolean;
  refresh: () => void;
}
