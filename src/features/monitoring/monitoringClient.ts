import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  DiskMetrics,
  ServerMetricsResult,
} from "./monitoring.types";

const SERVER_METRICS_COMMAND = "get_server_metrics";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
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

function optionalMessage(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }

  const message = value.trim();
  return message.length > 0 ? message.slice(0, 500) : null;
}

function normalizeDisk(value: unknown, index: number): DiskMetrics | null {
  if (!isRecord(value)) {
    return null;
  }

  const usedBytes = finiteNumber(value.usedBytes);
  const totalBytes = finiteNumber(value.totalBytes);

  if (usedBytes === null || totalBytes === null) {
    return null;
  }

  const rawName = typeof value.name === "string" ? value.name.trim() : "";
  const rawMountPoint =
    typeof value.mountPoint === "string" ? value.mountPoint.trim() : "";

  return {
    name: (rawName || rawMountPoint || `Disk ${index + 1}`).slice(0, 120),
    mountPoint: rawMountPoint.slice(0, 260),
    usedBytes: Math.min(usedBytes, totalBytes),
    totalBytes,
    percent: percentage(value.percent),
  };
}

function normalizeResult(value: unknown): ServerMetricsResult {
  if (!isRecord(value)) {
    throw new Error("The native monitoring response was not an object.");
  }

  if (value.status !== "online" && value.status !== "unavailable") {
    throw new Error("The native monitoring response had an invalid status.");
  }

  const sampledAt = finiteNumber(value.sampledAt);
  const disks = Array.isArray(value.disks)
    ? value.disks
        .slice(0, 64)
        .map(normalizeDisk)
        .filter((disk): disk is DiskMetrics => disk !== null)
    : [];

  return {
    status: value.status,
    sampledAt: sampledAt && sampledAt > 0 ? sampledAt : Date.now(),
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
    message: optionalMessage(value.message),
  };
}

export async function getServerMetrics(): Promise<ServerMetricsResult> {
  if (!isTauri()) {
    return {
      status: "unavailable",
      sampledAt: Date.now(),
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
      message: "Live monitoring requires the Tauri desktop runtime.",
    };
  }

  const response = await invoke<unknown>(SERVER_METRICS_COMMAND);
  return normalizeResult(response);
}

export function describeMonitoringError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message;
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim();
  }

  return "The server metrics request could not be completed.";
}
