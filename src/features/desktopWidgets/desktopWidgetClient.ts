import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  DesktopWidgetDisk,
  DesktopWidgetKind,
  DesktopWidgetMetrics,
  DesktopWidgetProviderState,
  DesktopWidgetRuntimeState,
  DesktopWidgetService,
  DesktopWidgetServiceStatus,
  DesktopWidgetSnapshot,
  DesktopWidgetTrendSample,
  DesktopWidgetUnavailableReason,
} from "./desktopWidget.types";

const SNAPSHOT_COMMAND = "get_desktop_widget_snapshot";
const SET_VISIBILITY_COMMAND = "set_desktop_widget_visibility";
const GET_RUNTIME_STATE_COMMAND = "get_desktop_widget_runtime_state";
const DISABLE_WIDGET_COMMAND = "disable_desktop_widget";
const MAX_SERVICES = 500;
const MAX_DISKS = 64;
const MAX_TREND_SAMPLES = 24;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseDesktopWidgetRuntimeState(
  value: unknown,
): DesktopWidgetRuntimeState {
  if (
    !isRecord(value) ||
    Object.keys(value).length !== 1 ||
    !Object.prototype.hasOwnProperty.call(value, "geometryRevision") ||
    typeof value.geometryRevision !== "string"
  ) {
    throw new Error("The desktop widget runtime state had an unexpected shape.");
  }

  const geometryRevision = value.geometryRevision.trim();
  if (
    geometryRevision.length === 0 ||
    geometryRevision.length > 256 ||
    geometryRevision !== value.geometryRevision
  ) {
    throw new Error("The desktop widget runtime state had an invalid revision.");
  }

  return { geometryRevision };
}

function finiteNumber(value: unknown, minimum = 0): number | null {
  return typeof value === "number" &&
    Number.isFinite(value) &&
    value >= minimum
    ? value
    : null;
}

function percentage(value: unknown): number | null {
  const number = finiteNumber(value);
  return number === null ? null : Math.min(number, 100);
}

function boundedString(
  value: unknown,
  fallback: string,
  maximumLength: number,
): string {
  if (typeof value !== "string") {
    return fallback;
  }

  const normalized = value.trim();
  return normalized.length > 0
    ? normalized.slice(0, maximumLength)
    : fallback;
}

function optionalMessage(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }

  const normalized = value.trim();
  return normalized.length > 0 ? normalized.slice(0, 500) : null;
}

function providerState(value: unknown): DesktopWidgetProviderState | null {
  return value === "configured" || value === "not-configured" ? value : null;
}

function unavailableReason(
  value: unknown,
): DesktopWidgetUnavailableReason | null {
  switch (value) {
    case "authentication":
    case "timeout":
    case "tls":
    case "connection":
    case "api-unavailable":
    case "invalid-data":
    case "not-configured":
      return value;
    default:
      return null;
  }
}

function normalizeDisk(value: unknown, index: number): DesktopWidgetDisk | null {
  if (!isRecord(value)) {
    return null;
  }

  const usedBytes = finiteNumber(value.usedBytes);
  const totalBytes = finiteNumber(value.totalBytes);

  if (usedBytes === null || totalBytes === null) {
    return null;
  }

  const mountPoint = boundedString(value.mountPoint, "", 260);

  return {
    name: boundedString(value.name, mountPoint || `Volume ${index + 1}`, 120),
    mountPoint,
    usedBytes: Math.min(usedBytes, totalBytes),
    totalBytes,
    percent: percentage(value.percent),
  };
}

function emptyMetrics(): DesktopWidgetMetrics {
  return {
    cpuPercent: null,
    memoryPercent: null,
    memoryUsedBytes: null,
    memoryTotalBytes: null,
    cpuTemperatureC: null,
    networkDownloadBytesPerSecond: null,
    networkUploadBytesPerSecond: null,
    uptimeSeconds: null,
    loadAverage1m: null,
    loadAverage5m: null,
    loadAverage15m: null,
    disks: [],
  };
}

function normalizeMetrics(value: unknown): DesktopWidgetMetrics {
  if (!isRecord(value)) {
    return emptyMetrics();
  }

  const disks = Array.isArray(value.disks)
    ? value.disks
        .slice(0, MAX_DISKS)
        .map(normalizeDisk)
        .filter((disk): disk is DesktopWidgetDisk => disk !== null)
    : [];

  return {
    cpuPercent: percentage(value.cpuPercent),
    memoryPercent: percentage(value.memoryPercent),
    memoryUsedBytes: finiteNumber(value.memoryUsedBytes),
    memoryTotalBytes: finiteNumber(value.memoryTotalBytes),
    cpuTemperatureC: finiteNumber(value.cpuTemperatureC, -273.15),
    networkDownloadBytesPerSecond: finiteNumber(
      value.networkDownloadBytesPerSecond,
    ),
    networkUploadBytesPerSecond: finiteNumber(
      value.networkUploadBytesPerSecond,
    ),
    uptimeSeconds: finiteNumber(value.uptimeSeconds),
    loadAverage1m: finiteNumber(value.loadAverage1m),
    loadAverage5m: finiteNumber(value.loadAverage5m),
    loadAverage15m: finiteNumber(value.loadAverage15m),
    disks,
  };
}

function serviceStatus(value: unknown): DesktopWidgetServiceStatus | null {
  return value === "online" || value === "offline" || value === "warning"
    ? value
    : null;
}

function normalizeService(
  value: unknown,
  index: number,
): DesktopWidgetService | null {
  if (!isRecord(value)) {
    return null;
  }

  const status = serviceStatus(value.status);
  if (status === null) {
    return null;
  }

  return {
    id: boundedString(value.id, `service-${index + 1}`, 64),
    name: boundedString(value.name, `Service ${index + 1}`, 80),
    status,
    message: optionalMessage(value.message),
  };
}

function normalizeTrend(value: unknown): DesktopWidgetTrendSample | null {
  if (!isRecord(value)) {
    return null;
  }

  const sampledAt = finiteNumber(value.sampledAt);
  if (sampledAt === null || sampledAt <= 0) {
    return null;
  }

  return {
    sampledAt,
    cpuPercent: percentage(value.cpuPercent),
    memoryPercent: percentage(value.memoryPercent),
    networkDownloadBytesPerSecond: finiteNumber(
      value.networkDownloadBytesPerSecond,
    ),
    networkUploadBytesPerSecond: finiteNumber(
      value.networkUploadBytesPerSecond,
    ),
    loadAverage1m: finiteNumber(value.loadAverage1m),
  };
}

function normalizeSnapshot(value: unknown): DesktopWidgetSnapshot {
  if (!isRecord(value)) {
    throw new Error("The desktop widget snapshot was not an object.");
  }

  if (value.status !== "online" && value.status !== "unavailable") {
    throw new Error("The desktop widget snapshot had an invalid status.");
  }

  const normalizedProviderState = providerState(value.providerState);
  if (normalizedProviderState === null) {
    throw new Error("The desktop widget snapshot had an invalid provider state.");
  }

  const normalizedReason = unavailableReason(value.reason);
  if (
    (value.reason !== null && normalizedReason === null) ||
    (normalizedProviderState === "not-configured" &&
      normalizedReason !== null &&
      normalizedReason !== "not-configured") ||
    (normalizedProviderState === "configured" &&
      normalizedReason === "not-configured")
  ) {
    throw new Error(
      "The desktop widget snapshot had inconsistent availability metadata.",
    );
  }

  const sampledAt = finiteNumber(value.sampledAt);
  const services = Array.isArray(value.services)
    ? value.services
        .slice(0, MAX_SERVICES)
        .map(normalizeService)
        .filter((service): service is DesktopWidgetService => service !== null)
    : [];
  const trends = Array.isArray(value.trends)
    ? value.trends
        .slice(-MAX_TREND_SAMPLES)
        .map(normalizeTrend)
        .filter(
          (sample): sample is DesktopWidgetTrendSample => sample !== null,
        )
    : [];

  return {
    status: value.status,
    providerState: normalizedProviderState,
    reason: normalizedReason,
    sampledAt: sampledAt && sampledAt > 0 ? sampledAt : Date.now(),
    serverName: boundedString(value.serverName, "Home Server", 80),
    serverAddress: boundedString(value.serverAddress, "—", 255),
    metrics: normalizeMetrics(value.metrics),
    services,
    trends,
    message: optionalMessage(value.message),
  };
}

function browserFallback(): DesktopWidgetSnapshot {
  return {
    status: "unavailable",
    providerState: "configured",
    reason: "api-unavailable",
    sampledAt: Date.now(),
    serverName: "Home Server",
    serverAddress: "—",
    metrics: emptyMetrics(),
    services: [],
    trends: [],
    message: "Live server data requires the Personal Hub desktop runtime.",
  };
}

export async function getDesktopWidgetSnapshot(
  kind: DesktopWidgetKind,
): Promise<DesktopWidgetSnapshot> {
  if (!isTauri()) {
    return browserFallback();
  }

  const response = await invoke<unknown>(SNAPSHOT_COMMAND, { kind });
  return normalizeSnapshot(response);
}

export async function setDesktopWidgetVisibility(
  kind: DesktopWidgetKind,
  visible: boolean,
): Promise<void> {
  if (!isTauri()) {
    return;
  }

  await invoke(SET_VISIBILITY_COMMAND, { kind, visible });
}

export async function getDesktopWidgetRuntimeState(
  kind: DesktopWidgetKind,
): Promise<DesktopWidgetRuntimeState> {
  if (!isTauri()) {
    return { geometryRevision: "preview" };
  }

  const response = await invoke<unknown>(GET_RUNTIME_STATE_COMMAND, { kind });
  return parseDesktopWidgetRuntimeState(response);
}

export async function disableDesktopWidget(
  kind: DesktopWidgetKind,
): Promise<void> {
  if (!isTauri()) {
    return;
  }

  await invoke(DISABLE_WIDGET_COMMAND, { kind });
}

export async function startDesktopWidgetDrag(): Promise<void> {
  if (!isTauri()) {
    return;
  }

  await getCurrentWindow().startDragging();
}

export function describeDesktopWidgetError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.slice(0, 500);
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }

  return "The server widget request could not be completed.";
}
