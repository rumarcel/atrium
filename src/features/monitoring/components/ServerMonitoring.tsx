import { useId, useState, type ComponentType, type SVGProps } from "react";
import {
  CpuIcon,
  MemoryIcon,
  NetworkIcon,
  RefreshIcon,
  ChevronIcon,
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
  /** Refreshed alongside the metrics, so the dashboard needs one button. */
  onRefresh?: () => void;
}

/// A metric only earns colour when it crosses a threshold. The disk level
/// matches the one the native notification runtime already alerts on, so the
/// dashboard and the toast never disagree about what counts as a problem.
const WARNING_AT = { cpu: 95, memory: 90, disk: 90, temperature: 85 } as const;

interface StatusMetricProps {
  Icon: ComponentType<SVGProps<SVGSVGElement>>;
  label: string;
  value: string;
  detail: string;
  percent?: number | null;
  warning?: boolean;
  detailTitle?: string;
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

function StatusMetric({
  Icon,
  label,
  value,
  detail,
  percent,
  warning = false,
  detailTitle,
}: StatusMetricProps) {
  const { t } = useTranslation();
  const safePercent = normalizedPercent(percent ?? null);
  const classes = warning ? "status-metric status-metric--warning" : "status-metric";

  return (
    <div className={classes}>
      <span className="status-metric__icon" aria-hidden="true">
        <Icon width={15} height={15} />
      </span>
      <span className="status-metric__label">{label}</span>
      <strong className="status-metric__value">{value}</strong>
      {safePercent !== null ? (
        <span
          className="status-metric__bar"
          role="progressbar"
          aria-label={t("monitoring.metricUsage", { metric: label })}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(safePercent)}
        >
          <span style={{ width: `${safePercent}%` }} />
        </span>
      ) : null}
      <span className="status-metric__detail" title={detailTitle ?? detail}>
        {detail}
      </span>
    </div>
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
  onRefresh,
  enabled,
  pollIntervalMs,
  staleAfterMs,
}: ServerMonitoringProps) {
  const { byteRate, bytes, percent, t, uptime: formatUptime } = useTranslation();
  const headingId = useId();
  const [volumesOpen, setVolumesOpen] = useState(false);
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
  const temperature = snapshot?.cpuTemperatureC ?? null;
  const diskWarning = (fullestDiskPercent ?? 0) >= WARNING_AT.disk;
  const memoryWarning = (memoryPercent ?? 0) >= WARNING_AT.memory;
  const cpuWarning =
    (cpuPercent ?? 0) >= WARNING_AT.cpu ||
    (temperature ?? 0) >= WARNING_AT.temperature;
  // The strip only raises its own tone for a threshold the user can act on.
  // A transport problem already shows through the connection tone.
  const tone =
    connection.tone === "online" && (diskWarning || memoryWarning || cpuWarning)
      ? "warning"
      : connection.tone;
  const classes = [
    "server-status",
    `server-status--${tone}`,
    snapshot === null ? "server-status--summary-only" : null,
    monitor.isStale ? "server-status--stale" : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <section className={classes} aria-labelledby={headingId}>
      <div
        className="server-status__identity"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        <span className="server-status__dot" aria-hidden="true" />
        <span className="server-status__icon" aria-hidden="true">
          <ServerIcon width={18} height={18} />
        </span>
        <span className="server-status__copy">
          <h2 id={headingId}>{connection.title}</h2>
          <small title={connection.description}>
            {hasUptime || monitor.isStale
              ? `${uptimeLabel} · ${uptime}`
              : connection.description}
          </small>
        </span>
      </div>

      {snapshot ? (
        <div
          className="server-status__metrics"
          aria-label={
            monitor.isStale
              ? t("monitoring.lastKnownMetrics")
              : t("monitoring.liveMetrics")
          }
          aria-busy={monitor.isRefreshing}
        >
          <StatusMetric
            Icon={CpuIcon}
            label={t("monitoring.cpu")}
            value={percent(cpuPercent)}
            detail={
              temperature === null
                ? t("monitoring.temperatureUnavailable")
                : t("monitoring.temperature", {
                    temperature: `${Math.round(temperature)} °C`,
                  })
            }
            percent={cpuPercent}
            warning={cpuWarning}
          />
          <StatusMetric
            Icon={MemoryIcon}
            label={t("monitoring.memory")}
            value={percent(memoryPercent)}
            detail={t("monitoring.memoryUsage", {
              used: bytes(snapshot.memoryUsedBytes),
              total: bytes(snapshot.memoryTotalBytes),
            })}
            percent={memoryPercent}
            warning={memoryWarning}
          />
          <StatusMetric
            Icon={StorageIcon}
            label={t("monitoring.storage")}
            value={percent(fullestDiskPercent)}
            detail={
              fullestDisk === null
                ? t("monitoring.diskDataUnavailable")
                : t("monitoring.diskUsage", {
                    used: bytes(fullestDisk.usedBytes),
                    total: bytes(fullestDisk.totalBytes),
                  })
            }
            detailTitle={fullestDisk?.name ?? undefined}
            percent={fullestDiskPercent}
            warning={diskWarning}
          />

          <StatusMetric
            Icon={NetworkIcon}
            label={t("monitoring.network")}
            value={byteRate(snapshot.networkDownloadBytesPerSecond)}
            detail={t("monitoring.networkRate", {
              upload: byteRate(snapshot.networkUploadBytesPerSecond),
            })}
          />
        </div>
      ) : null}

      {disks.length > 1 ? (
        <button
          className="server-status__volumes-toggle"
          type="button"
          onClick={() => setVolumesOpen((open) => !open)}
          aria-expanded={volumesOpen}
          aria-controls={`${headingId}-volumes`}
          title={t(volumesOpen ? "monitoring.hideVolumes" : "monitoring.showVolumes")}
        >
          <span>
            {t(
              disks.length === 1
                ? "monitoring.volumeCountOne"
                : "monitoring.volumeCountOther",
              { count: String(disks.length) },
            )}
          </span>
          <ChevronIcon width={13} height={13} />
        </button>
      ) : null}

      <button
        className="server-status__refresh"
        type="button"
        onClick={() => {
          monitor.refresh();
          onRefresh?.();
        }}
        disabled={
          monitor.isRefreshing || monitor.isPaused || enabled === false
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

      {volumesOpen && disks.length > 1 ? (
        <ul className="server-status__volumes" id={`${headingId}-volumes`}>
          {disks.map((disk) => {
            const usage = derivedPercent(
              disk.percent,
              disk.usedBytes,
              disk.totalBytes,
            );
            const full = (usage ?? 0) >= WARNING_AT.disk;

            return (
              <li
                key={disk.mountPoint || disk.name}
                className={full ? "server-volume server-volume--warning" : "server-volume"}
              >
                <span className="server-volume__name" title={disk.mountPoint || disk.name}>
                  {disk.name}
                </span>
                <span className="server-volume__bar">
                  <span style={{ width: `${usage ?? 0}%` }} />
                </span>
                <strong className="server-volume__value">{percent(usage)}</strong>
                <span className="server-volume__detail">
                  {t("monitoring.diskUsage", {
                    used: bytes(disk.usedBytes),
                    total: bytes(disk.totalBytes),
                  })}
                </span>
              </li>
            );
          })}
        </ul>
      ) : null}
    </section>
  );
}
