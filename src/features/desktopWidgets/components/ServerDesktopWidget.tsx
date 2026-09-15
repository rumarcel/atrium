import type { DesktopWidgetMonitor } from "../desktopWidget.types";
import {
  derivedPercent,
  normalizedPercent,
} from "../desktopWidgetFormatters";
import { useTranslation } from "../../i18n";
import {
  WidgetArrowDownIcon,
  WidgetArrowUpIcon,
  WidgetServerIcon,
} from "./DesktopWidgetIcons";
import { DesktopWidgetShell } from "./DesktopWidgetShell";
import { DesktopWidgetSparkline } from "./DesktopWidgetSparkline";

interface ServerDesktopWidgetProps {
  monitor: DesktopWidgetMonitor;
}

interface PercentageMetricProps {
  label: string;
  value: number | null;
  detail: string;
}

function PercentageMetric({ label, value, detail }: PercentageMetricProps) {
  const { percent: formatPercent, t } = useTranslation();
  const safePercent = normalizedPercent(value);

  return (
    <article className="desktop-widget-metric">
      <span>{label}</span>
      <strong>{formatPercent(safePercent)}</strong>
      <small>{detail}</small>
      {safePercent !== null ? (
        <div
          className="desktop-widget-progress"
          role="progressbar"
          aria-label={t("widget.metricUsage", { metric: label })}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(safePercent)}
        >
          <span style={{ width: `${safePercent}%` }} />
        </div>
      ) : null}
    </article>
  );
}

export function ServerDesktopWidget({ monitor }: ServerDesktopWidgetProps) {
  const { byteRate, bytes, duration, number, t } = useTranslation();
  const snapshot = monitor.snapshot;
  const metrics = snapshot?.metrics ?? null;
  const memoryPercent = derivedPercent(
    metrics?.memoryPercent ?? null,
    metrics?.memoryUsedBytes ?? null,
    metrics?.memoryTotalBytes ?? null,
  );
  const trends = snapshot?.trends ?? [];

  return (
    <DesktopWidgetShell
      kind="server"
      title={snapshot?.serverName ?? t("widget.homeServer")}
      subtitle={snapshot?.serverAddress ?? "—"}
      monitor={monitor}
      icon={<WidgetServerIcon />}
    >
      <div className="desktop-widget-metric-grid">
        <PercentageMetric
          label={t("monitoring.cpu")}
          value={metrics?.cpuPercent ?? null}
          detail={
            metrics?.cpuTemperatureC === null || !metrics
              ? t("widget.temperatureUnavailable")
              : t("widget.temperature", {
                  temperature: `${number(Math.round(metrics.cpuTemperatureC))} °C`,
                })
          }
        />
        <PercentageMetric
          label={t("monitoring.memory")}
          value={memoryPercent}
          detail={t("widget.memoryUsage", {
            used: bytes(metrics?.memoryUsedBytes ?? null),
            total: bytes(metrics?.memoryTotalBytes ?? null),
          })}
        />
        <article className="desktop-widget-metric desktop-widget-metric--network">
          <span>{t("monitoring.network")}</span>
          <strong>{byteRate(metrics?.networkDownloadBytesPerSecond ?? null)}</strong>
          <small className="desktop-widget-network-detail">
            <span>
              <WidgetArrowDownIcon />
              {t("widget.down")}
            </span>
            <span>
              <WidgetArrowUpIcon />
              {t("widget.networkUpload", {
                upload: byteRate(metrics?.networkUploadBytesPerSecond ?? null),
              })}
            </span>
          </small>
        </article>
      </div>

      <div
        className="desktop-widget-facts"
        aria-label={t("widget.serverUptimeAndLoad")}
      >
        <div>
          <span>{t("widget.uptime")}</span>
          <strong>
            {duration(metrics?.uptimeSeconds ?? null, {
              maximumParts: 2,
              smallestUnit: "minute",
              unitDisplay: "narrow",
            })}
          </strong>
        </div>
        <div>
          <span>{t("widget.loadAverage")}</span>
          <strong>
            {number(metrics?.loadAverage1m ?? null, {
              maximumFractionDigits:
                (metrics?.loadAverage1m ?? 0) >= 10 ? 1 : 2,
            })}
          </strong>
          <small>
            {number(metrics?.loadAverage5m ?? null, {
              maximumFractionDigits:
                (metrics?.loadAverage5m ?? 0) >= 10 ? 1 : 2,
            })}{" "}
            /{" "}
            {number(metrics?.loadAverage15m ?? null, {
              maximumFractionDigits:
                (metrics?.loadAverage15m ?? 0) >= 10 ? 1 : 2,
            })}
          </small>
        </div>
      </div>

      <div className="desktop-widget-trends" aria-label={t("widget.recentTrends")}>
        <article>
          <span>{t("monitoring.cpu")}</span>
          <DesktopWidgetSparkline
            values={trends.map((sample) => sample.cpuPercent)}
            label={t("widget.recentCpuTrend")}
          />
        </article>
        <article>
          <span>{t("monitoring.memory")}</span>
          <DesktopWidgetSparkline
            values={trends.map((sample) => sample.memoryPercent)}
            label={t("widget.recentMemoryTrend")}
            tone="violet"
          />
        </article>
        <article>
          <span>{t("monitoring.network")}</span>
          <DesktopWidgetSparkline
            values={trends.map(
              (sample) => sample.networkDownloadBytesPerSecond,
            )}
            label={t("widget.recentNetworkTrend")}
            tone="green"
          />
        </article>
      </div>
    </DesktopWidgetShell>
  );
}
