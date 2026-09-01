import type {
  DesktopWidgetDisk,
  DesktopWidgetMonitor,
} from "../desktopWidget.types";
import {
  derivedPercent,
} from "../desktopWidgetFormatters";
import { useTranslation } from "../../i18n";
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
  const { bytes, percent: formatPercent, t } = useTranslation();
  const percent = diskPercent(disk);
  const tone = diskTone(percent);

  return (
    <article className={`desktop-widget-disk desktop-widget-disk--${tone}`}>
      <div className="desktop-widget-disk__heading">
        <div>
          <strong title={disk.name}>{disk.name}</strong>
          <small title={disk.mountPoint}>
            {disk.mountPoint || t("widget.serverVolume")}
          </small>
        </div>
        <span>{formatPercent(percent)}</span>
      </div>
      <div
        className="desktop-widget-progress"
        role="progressbar"
        aria-label={t("widget.storageUsage", { name: disk.name })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent === null ? undefined : Math.round(percent)}
      >
        <span style={{ width: `${percent ?? 0}%` }} />
      </div>
      <div className="desktop-widget-disk__footer">
        <span>
          {t("widget.usedOf", {
            used: bytes(disk.usedBytes),
            total: bytes(disk.totalBytes),
          })}
        </span>
        {tone === "normal" ? null : (
          <span className="desktop-widget-disk__warning">
            <WidgetAlertIcon />
            {tone === "critical"
              ? t("widget.critical")
              : t("widget.lowSpace")}
          </span>
        )}
      </div>
    </article>
  );
}

export function StorageDesktopWidget({ monitor }: StorageDesktopWidgetProps) {
  const { number, t } = useTranslation();
  const snapshot = monitor.snapshot;
  const disks = snapshot?.metrics.disks ?? [];
  const warningCount = disks.filter((disk) => {
    const percent = diskPercent(disk);
    return percent !== null && percent >= 85;
  }).length;

  return (
    <DesktopWidgetShell
      kind="storage"
      title={t("widget.serverStorage")}
      subtitle={`${snapshot?.serverName ?? t("widget.homeServer")} · ${
        snapshot?.serverAddress ?? "192.168.1.10"
      }`}
      monitor={monitor}
      icon={<WidgetStorageIcon />}
    >
      <div className="desktop-widget-storage-summary">
        <span>
          {t(
            disks.length === 1
              ? "widget.volumeCountOne"
              : "widget.volumeCountOther",
            { count: number(disks.length) },
          )}
        </span>
        <span
          className={
            warningCount > 0
              ? "desktop-widget-storage-summary__warning"
              : "desktop-widget-storage-summary__healthy"
          }
        >
          {warningCount > 0 ? <WidgetAlertIcon /> : <WidgetCheckIcon />}
          {disks.length === 0
            ? t("widget.awaitingCapacity")
            : warningCount > 0
              ? t(
                  warningCount === 1
                    ? "widget.warningCountOne"
                    : "widget.warningCountOther",
                  { count: number(warningCount) },
                )
              : t("widget.capacityHealthy")}
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
            <strong>{t("widget.noVolumeData")}</strong>
            <span>{t("widget.noVolumeDataDescription")}</span>
          </div>
        )}
      </div>
    </DesktopWidgetShell>
  );
}
