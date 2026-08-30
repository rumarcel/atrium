import type { DesktopWidgetMonitor } from "../desktopWidget.types";
import {
  derivedPercent,
  formatBytes,
  formatDuration,
  formatLoad,
  formatPercent,
  formatRate,
  normalizedPercent,
} from "../desktopWidgetFormatters";
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
  const percent = normalizedPercent(value);

  return (
    <article className="desktop-widget-metric">
      <span>{label}</span>
      <strong>{formatPercent(percent)}</strong>
      <small>{detail}</small>
      {percent !== null ? (
        <div
          className="desktop-widget-progress"
          role="progressbar"
          aria-label={`${label} usage`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(percent)}
        >
          <span style={{ width: `${percent}%` }} />
        </div>
      ) : null}
    </article>
  );
}

export function ServerDesktopWidget({ monitor }: ServerDesktopWidgetProps) {
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
      title={snapshot?.serverName ?? "Home Server"}
      subtitle={snapshot?.serverAddress ?? "192.168.1.10"}
      monitor={monitor}
      icon={<WidgetServerIcon />}
    >
      <div className="desktop-widget-metric-grid">
        <PercentageMetric
          label="CPU"
          value={metrics?.cpuPercent ?? null}
          detail={
            metrics?.cpuTemperatureC === null || !metrics
              ? "Temperature unavailable"
              : `${Math.round(metrics.cpuTemperatureC)} °C`
          }
        />
        <PercentageMetric
          label="Memory"
          value={memoryPercent}
          detail={`${formatBytes(metrics?.memoryUsedBytes ?? null)} / ${formatBytes(
            metrics?.memoryTotalBytes ?? null,
          )}`}
        />
        <article className="desktop-widget-metric desktop-widget-metric--network">
          <span>Network</span>
          <strong>{formatRate(metrics?.networkDownloadBytesPerSecond ?? null)}</strong>
          <small className="desktop-widget-network-detail">
            <span>
              <WidgetArrowDownIcon />
              Down
            </span>
            <span>
              <WidgetArrowUpIcon />
              {formatRate(metrics?.networkUploadBytesPerSecond ?? null)} up
            </span>
          </small>
        </article>
      </div>

      <div className="desktop-widget-facts" aria-label="Server uptime and load">
        <div>
          <span>Uptime</span>
          <strong>{formatDuration(metrics?.uptimeSeconds ?? null)}</strong>
        </div>
        <div>
          <span>Load average</span>
          <strong>{formatLoad(metrics?.loadAverage1m ?? null)}</strong>
          <small>
            {formatLoad(metrics?.loadAverage5m ?? null)} / {formatLoad(
              metrics?.loadAverage15m ?? null,
            )}
          </small>
        </div>
      </div>

      <div className="desktop-widget-trends" aria-label="Recent server trends">
        <article>
          <span>CPU</span>
          <DesktopWidgetSparkline
            values={trends.map((sample) => sample.cpuPercent)}
            label="Recent CPU usage trend"
          />
        </article>
        <article>
          <span>Memory</span>
          <DesktopWidgetSparkline
            values={trends.map((sample) => sample.memoryPercent)}
            label="Recent memory usage trend"
            tone="violet"
          />
        </article>
        <article>
          <span>Network</span>
          <DesktopWidgetSparkline
            values={trends.map(
              (sample) => sample.networkDownloadBytesPerSecond,
            )}
            label="Recent network download trend"
            tone="green"
          />
        </article>
      </div>
    </DesktopWidgetShell>
  );
}
