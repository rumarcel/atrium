import { useId, type ComponentType, type ReactNode, type SVGProps } from "react";
import {
  CpuIcon,
  MemoryIcon,
  NetworkIcon,
  RefreshIcon,
  ServerIcon,
  StorageIcon,
} from "../../../components/icons/AppIcons";
import { useTranslation, type Translator } from "../../i18n";
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
  const { t } = useTranslation();
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
          aria-label={t("monitoring.metricUsage", { metric: label })}
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

function connectionCopy(monitor: ServerMetricsMonitor, t: Translator) {
  if (monitor.status === "loading") {
    return {
      tone: "loading",
      eyebrow: t("monitoring.connectingEyebrow"),
      title: t("monitoring.connectingTitle"),
      description: t("monitoring.connectingDescription"),
    } as const;
  }

  if (monitor.providerState === "not-configured") {
    return {
      tone: "inactive",
      eyebrow: t("monitoring.optionalEyebrow"),
      title: t("monitoring.notConfiguredTitle"),
      description: t("monitoring.notConfiguredDescription"),
    } as const;
  }

  if (monitor.status === "online" && !monitor.isStale) {
    if (monitor.message) {
      return {
        tone: "warning",
        eyebrow: t("monitoring.partialEyebrow"),
        title: t("monitoring.partialTitle"),
        description: monitor.message,
      } as const;
    }

    return {
      tone: "online",
      eyebrow: t("monitoring.liveEyebrow"),
      title: t("monitoring.onlineTitle"),
      description: monitor.message ?? t("monitoring.onlineDescription"),
    } as const;
  }

  if (monitor.snapshot) {
    return {
      tone: "stale",
      eyebrow: t("monitoring.staleEyebrow"),
      title: t("monitoring.interruptedTitle"),
      description: monitor.message ?? t("monitoring.interruptedDescription"),
    } as const;
  }

  return {
    tone: "unavailable",
    eyebrow: t("monitoring.unavailableEyebrow"),
    title: t("monitoring.unavailableTitle"),
    description:
      monitor.message ??
      t("monitoring.unavailableDescription"),
  } as const;
}

export function ServerMonitoring({
  className,
  enabled,
  pollIntervalMs,
  staleAfterMs,
}: ServerMonitoringProps) {
  const { byteRate, bytes, percent, t, uptime: formatUptime } = useTranslation();
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
  const connection = connectionCopy(monitor, t);
  const uptime = formatUptime(snapshot?.uptimeSeconds ?? null, {
    placeholder: t("monitoring.uptimeUnavailable"),
  });
  const hasUptime = snapshot?.uptimeSeconds != null;
  const uptimeLabel = monitor.isStale && hasUptime
    ? t("monitoring.lastKnownUptime")
    : t("monitoring.serverUptime");
  const classes = [
    "server-monitoring",
    snapshot === null ? "server-monitoring--summary-only" : null,
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
            disabled={
              monitor.isRefreshing ||
              monitor.isPaused ||
              monitor.providerState === "not-configured" ||
              enabled === false
            }
            aria-label={t("monitoring.refresh")}
            title={t("monitoring.refresh")}
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

        <div
          className={`monitoring-summary__uptime monitoring-summary__uptime--${connection.tone}`}
          aria-label={`${uptimeLabel}: ${uptime}`}
        >
          <span>{uptimeLabel}</span>
          <strong>{uptime}</strong>
        </div>
      </article>

      {snapshot ? (
        <div
          className="monitoring-metrics"
          aria-label={
            monitor.isStale
              ? t("monitoring.lastKnownMetrics")
              : t("monitoring.liveMetrics")
          }
          aria-busy={monitor.isRefreshing}
        >
          <MetricShell
            Icon={CpuIcon}
            label={t("monitoring.cpu")}
            value={percent(cpuPercent)}
            detail={
              snapshot.cpuTemperatureC === null
                ? t("monitoring.temperatureUnavailable")
                : t("monitoring.temperature", {
                    temperature: `${Math.round(snapshot.cpuTemperatureC)} °C`,
                  })
            }
            percent={cpuPercent}
          />

          <MetricShell
            Icon={MemoryIcon}
            label={t("monitoring.memory")}
            value={percent(memoryPercent)}
            detail={t("monitoring.memoryUsage", {
              used: bytes(snapshot.memoryUsedBytes),
              total: bytes(snapshot.memoryTotalBytes),
            })}
            percent={memoryPercent}
          />

          <MetricShell
            Icon={NetworkIcon}
            label={t("monitoring.network")}
            value={byteRate(snapshot.networkDownloadBytesPerSecond)}
            detail={t("monitoring.networkRate", {
              upload: byteRate(snapshot.networkUploadBytesPerSecond),
            })}
          />

          <MetricShell
            Icon={StorageIcon}
            label={t("monitoring.storage")}
            value={percent(fullestDiskPercent)}
            detail={
              fullestDisk === null
                ? t("monitoring.diskDataUnavailable")
                : t("monitoring.diskUsage", {
                    name: fullestDisk.name,
                    used: bytes(fullestDisk.usedBytes),
                    total: bytes(fullestDisk.totalBytes),
                  })
            }
            percent={fullestDiskPercent}
          >
            {disks.length > 0 ? (
              <span className="monitoring-metric__disk-count">
                {t(
                  disks.length === 1
                    ? "monitoring.volumeCountOne"
                    : "monitoring.volumeCountOther",
                  { count: String(disks.length) },
                )}
              </span>
            ) : null}
          </MetricShell>
        </div>
      ) : null}
    </section>
  );
}
