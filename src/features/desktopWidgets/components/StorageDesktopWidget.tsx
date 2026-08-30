import type {
  DesktopWidgetDisk,
  DesktopWidgetMonitor,
} from "../desktopWidget.types";
import {
  derivedPercent,
  formatBytes,
  formatPercent,
} from "../desktopWidgetFormatters";
import {
  WidgetAlertIcon,
  WidgetCheckIcon,
  WidgetStorageIcon,
} from "./DesktopWidgetIcons";
import { DesktopWidgetShell } from "./DesktopWidgetShell";

interface StorageDesktopWidgetProps {
  monitor: DesktopWidgetMonitor;
}

type DiskTone = "normal" | "warning" | "critical";

function diskPercent(disk: DesktopWidgetDisk): number | null {
  return derivedPercent(disk.percent, disk.usedBytes, disk.totalBytes);
}

function diskTone(percent: number | null): DiskTone {
  if (percent !== null && percent >= 95) {
    return "critical";
  }

  if (percent !== null && percent >= 85) {
    return "warning";
  }

  return "normal";
}

function DiskRow({ disk }: { disk: DesktopWidgetDisk }) {
  const percent = diskPercent(disk);
  const tone = diskTone(percent);

  return (
    <article className={`desktop-widget-disk desktop-widget-disk--${tone}`}>
      <div className="desktop-widget-disk__heading">
        <div>
          <strong title={disk.name}>{disk.name}</strong>
          <small title={disk.mountPoint}>{disk.mountPoint || "Server volume"}</small>
        </div>
        <span>{formatPercent(percent)}</span>
      </div>
      <div
        className="desktop-widget-progress"
        role="progressbar"
        aria-label={`${disk.name} storage usage`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent === null ? undefined : Math.round(percent)}
      >
        <span style={{ width: `${percent ?? 0}%` }} />
      </div>
      <div className="desktop-widget-disk__footer">
        <span>
          {formatBytes(disk.usedBytes)} of {formatBytes(disk.totalBytes)}
        </span>
        {tone === "normal" ? null : (
          <span className="desktop-widget-disk__warning">
            <WidgetAlertIcon />
            {tone === "critical" ? "Critical" : "Low space"}
          </span>
        )}
      </div>
    </article>
  );
}

export function StorageDesktopWidget({ monitor }: StorageDesktopWidgetProps) {
  const snapshot = monitor.snapshot;
  const disks = snapshot?.metrics.disks ?? [];
  const warningCount = disks.filter((disk) => {
    const percent = diskPercent(disk);
    return percent !== null && percent >= 85;
  }).length;

  return (
    <DesktopWidgetShell
      kind="storage"
      title="Server storage"
      subtitle={`${snapshot?.serverName ?? "Home Server"} · ${
        snapshot?.serverAddress ?? "192.168.1.10"
      }`}
      monitor={monitor}
      icon={<WidgetStorageIcon />}
    >
      <div className="desktop-widget-storage-summary">
        <span>{disks.length} {disks.length === 1 ? "volume" : "volumes"}</span>
        <span
          className={
            warningCount > 0
              ? "desktop-widget-storage-summary__warning"
              : "desktop-widget-storage-summary__healthy"
          }
        >
          {warningCount > 0 ? <WidgetAlertIcon /> : <WidgetCheckIcon />}
          {disks.length === 0
            ? "Awaiting capacity"
            : warningCount > 0
            ? `${warningCount} ${warningCount === 1 ? "warning" : "warnings"}`
            : "Capacity healthy"}
        </span>
      </div>

      <div className="desktop-widget-disk-list">
        {disks.length > 0 ? (
          disks.map((disk, index) => (
            <DiskRow key={`${disk.mountPoint}:${disk.name}:${index}`} disk={disk} />
          ))
        ) : (
          <div className="desktop-widget-empty">
            <WidgetStorageIcon />
            <strong>No volume data</strong>
            <span>Waiting for storage metrics from the server.</span>
          </div>
        )}
      </div>
    </DesktopWidgetShell>
  );
}
