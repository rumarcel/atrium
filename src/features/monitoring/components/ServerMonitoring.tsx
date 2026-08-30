import { useId, type ComponentType, type ReactNode, type SVGProps } from "react";
import {
  CpuIcon,
  MemoryIcon,
  NetworkIcon,
  RefreshIcon,
  ServerIcon,
  StorageIcon,
} from "../../../components/icons/AppIcons";
import { useServerMetrics } from "../hooks/useServerMetrics";
import type {
  DiskMetrics,
  ServerMetricsMonitor,
  UseServerMetricsOptions,
} from "../monitoring.types";

export interface ServerMonitoringProps extends UseServerMetricsOptions {
  className?: string;
}

interface MetricShellProps {
  Icon: ComponentType<SVGProps<SVGSVGElement>>;
  label: string;
  value: string;
  detail: string;
  percent?: number | null;
  children?: ReactNode;
}

const byteUnits = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

function formatBytes(bytes: number | null, fractionDigits = 1): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) {
    return "—";
  }

  if (bytes === 0) {
    return "0 B";
  }

  const unitIndex = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    byteUnits.length - 1,
  );
  const value = bytes / 1024 ** unitIndex;
  const digits = value >= 100 || unitIndex === 0 ? 0 : fractionDigits;
  return `${value.toFixed(digits)} ${byteUnits[unitIndex]}`;
}

function formatRate(bytesPerSecond: number | null): string {
  const bytes = formatBytes(bytesPerSecond);
  return bytes === "—" ? bytes : `${bytes}/s`;
}

function formatPercent(value: number | null): string {
  return value === null ? "—" : `${Math.round(value)}%`;
}

function normalizedPercent(value: number | null): number | null {
  return value === null || !Number.isFinite(value)
    ? null
    : Math.max(0, Math.min(100, value));
}

function derivedPercent(
  explicitPercent: number | null,
  used: number | null,
  total: number | null,
): number | null {
  const normalized = normalizedPercent(explicitPercent);
  if (normalized !== null) {
    return normalized;
  }

  if (used === null || total === null || total <= 0) {
    return null;
  }

  return normalizedPercent((used / total) * 100);
}

function mostUsedDisk(disks: readonly DiskMetrics[]) {
  if (disks.length === 0) {
    return null;
  }

  return disks.reduce((mostUsed, disk) =>
    (derivedPercent(disk.percent, disk.usedBytes, disk.totalBytes) ?? -1) >
    (derivedPercent(
      mostUsed.percent,
      mostUsed.usedBytes,
      mostUsed.totalBytes,
    ) ?? -1)
      ? disk
      : mostUsed,
  );
}

function MetricShell({
  Icon,
  label,
  value,
  detail,
  percent,
  children,
}: MetricShellProps) {
  const safePercent = normalizedPercent(percent ?? null);

  return (
    <article className="monitoring-metric">
      <div className="monitoring-metric__heading">
        <span className="monitoring-metric__icon" aria-hidden="true">
          <Icon width={17} height={17} />
        </span>
        <span>{label}</span>
      </div>
      <strong>{value}</strong>
      <small>{detail}</small>
      {safePercent !== null ? (
        <div
          className="monitoring-progress"
          role="progressbar"
          aria-label={`${label} usage`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(safePercent)}
        >
          <span style={{ width: `${safePercent}%` }} />
        </div>
      ) : null}
      {children}
    </article>
  );
}

function connectionCopy(monitor: ServerMetricsMonitor) {
  if (monitor.status === "loading") {
    return {
      tone: "loading",
      eyebrow: "Connecting",
      title: "Reaching Glances",
      description: "Waiting for the first server sample.",
    } as const;
  }

  if (monitor.status === "online" && !monitor.isStale) {
    if (monitor.message) {
      return {
        tone: "warning",
        eyebrow: "Partial",
        title: "Some metrics unavailable",
        description: monitor.message,
      } as const;
    }

    return {
      tone: "online",
      eyebrow: "Live",
      title: "Server online",
      description: monitor.message ?? "System metrics are updating automatically.",
    } as const;
  }

  if (monitor.snapshot) {
    return {
      tone: "stale",
      eyebrow: "Stale data",
      title: "Live metrics interrupted",
      description: monitor.message ?? "Showing the last successful server sample.",
    } as const;
  }

  return {
    tone: "unavailable",
    eyebrow: "Unavailable",
    title: "Monitoring unavailable",
    description: monitor.message ?? "Glances did not return server metrics.",
  } as const;
}

export function ServerMonitoring({
  className,
  enabled,
  pollIntervalMs,
  staleAfterMs,
}: ServerMonitoringProps) {
  const headingId = useId();
  const monitor = useServerMetrics({ enabled, pollIntervalMs, staleAfterMs });
  const snapshot = monitor.snapshot;
  const cpuPercent = snapshot?.cpuPercent ?? null;
  const memoryPercent = derivedPercent(
    snapshot?.memoryPercent ?? null,
    snapshot?.memoryUsedBytes ?? null,
    snapshot?.memoryTotalBytes ?? null,
  );
  const disks = snapshot?.disks ?? [];
  const fullestDisk = mostUsedDisk(disks);
  const fullestDiskPercent = fullestDisk
    ? derivedPercent(
        fullestDisk.percent,
        fullestDisk.usedBytes,
        fullestDisk.totalBytes,
      )
    : null;
  const connection = connectionCopy(monitor);
  const classes = [
    "server-monitoring",
    monitor.isStale ? "server-monitoring--stale" : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <section className={classes} aria-labelledby={headingId}>
      <article className="monitoring-summary">
        <div className="monitoring-summary__topline">
          <div
            className={`monitoring-summary__icon monitoring-summary__icon--${connection.tone}`}
            aria-hidden="true"
          >
            <ServerIcon width={22} height={22} />
          </div>
          <button
            className="monitoring-summary__refresh"
            type="button"
            onClick={monitor.refresh}
            disabled={monitor.isRefreshing || monitor.isPaused || enabled === false}
            aria-label="Refresh server metrics"
            title="Refresh server metrics"
          >
            <RefreshIcon
              className={monitor.isRefreshing ? "is-spinning" : undefined}
              width={15}
              height={15}
            />
          </button>
        </div>

        <div
          className="monitoring-summary__content"
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          <div
            className={`monitoring-summary__eyebrow monitoring-summary__eyebrow--${connection.tone}`}
          >
            <span className="monitoring-status-dot" aria-hidden="true" />
            {connection.eyebrow}
          </div>
          <h2 id={headingId}>{connection.title}</h2>
          <p title={connection.description}>{connection.description}</p>
        </div>

      </article>

      <div
        className="monitoring-metrics"
        aria-label={monitor.isStale ? "Last known server metrics" : "Live server metrics"}
        aria-busy={monitor.isRefreshing}
      >
        <MetricShell
          Icon={CpuIcon}
          label="CPU"
          value={formatPercent(cpuPercent)}
          detail={
            snapshot?.cpuTemperatureC === null || !snapshot
              ? "Temperature unavailable"
              : `${Math.round(snapshot.cpuTemperatureC)} °C temperature`
          }
          percent={cpuPercent}
        />

        <MetricShell
          Icon={MemoryIcon}
          label="Memory"
          value={formatPercent(memoryPercent)}
          detail={
            snapshot
              ? `${formatBytes(snapshot.memoryUsedBytes)} of ${formatBytes(snapshot.memoryTotalBytes)}`
              : "Used and total unavailable"
          }
          percent={memoryPercent}
        />

        <MetricShell
          Icon={NetworkIcon}
          label="Network"
          value={formatRate(snapshot?.networkDownloadBytesPerSecond ?? null)}
          detail={`Down · ${formatRate(snapshot?.networkUploadBytesPerSecond ?? null)} up`}
        />

        <MetricShell
          Icon={StorageIcon}
          label="Storage"
          value={formatPercent(fullestDiskPercent)}
          detail={
            fullestDisk === null
              ? "Disk data unavailable"
              : `${fullestDisk.name} · ${formatBytes(fullestDisk.usedBytes)} of ${formatBytes(fullestDisk.totalBytes)}`
          }
          percent={fullestDiskPercent}
        >
          {disks.length > 0 ? (
            <span className="monitoring-metric__disk-count">
              {disks.length} {disks.length === 1 ? "volume" : "volumes"}
            </span>
          ) : null}
        </MetricShell>
      </div>
    </section>
  );
}
