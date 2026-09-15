import { useState } from "react";
import { ChevronIcon } from "../components/icons/AppIcons";
import { useTranslation } from "../features/i18n";
import type { ServerMetricsMonitor } from "../features/monitoring/monitoring.types";
import type { DownloadCenterMonitor } from "../features/downloads/downloads.types";

interface ActivityBarProps {
  monitor: ServerMetricsMonitor;
  downloads: DownloadCenterMonitor;
}

function usage(
  percentValue: number | null,
  used: number | null,
  total: number | null,
): number | null {
  if (percentValue !== null && Number.isFinite(percentValue)) {
    return Math.max(0, Math.min(100, percentValue));
  }
  if (used === null || total === null || total <= 0) {
    return null;
  }
  return Math.max(0, Math.min(100, (used / total) * 100));
}

/**
 * The numbers, at the bottom.
 *
 * They used to sit at the top of the screen in four cards, which spent the
 * most valuable space on something nobody opens this application to read. The
 * verdict in the header says whether they need looking at; this is where you
 * look. Volumes expand because a server can have several and only the fullest
 * one belongs on a single line.
 */
export function ActivityBar({ monitor, downloads }: ActivityBarProps) {
  const { byteRate, bytes, percent, t } = useTranslation();
  const [open, setOpen] = useState(false);
  const snapshot = monitor.snapshot;
  const active = downloads.snapshot?.items ?? [];
  const downloadSpeed = downloads.snapshot?.totalDownloadSpeedBytesPerSecond ?? 0;
  // Downloads are a reading like any other here. With nothing transferring they
  // take one slot instead of a titled panel announcing that it is empty.
  const hasDownloads = downloads.providerState === "configured";

  if (!snapshot && !hasDownloads) {
    return null;
  }

  const disks = snapshot?.disks ?? [];
  const fullest = disks.reduce<(typeof disks)[number] | null>(
    (worst, disk) =>
      worst === null ||
      (usage(disk.percent, disk.usedBytes, disk.totalBytes) ?? -1) >
        (usage(worst.percent, worst.usedBytes, worst.totalBytes) ?? -1)
        ? disk
        : worst,
    null,
  );

  const readings = !snapshot ? [] : [
    {
      key: "cpu",
      label: t("monitoring.cpu"),
      value: percent(snapshot!.cpuPercent),
      note:
        snapshot.cpuTemperatureC === null
          ? null
          : `${Math.round(snapshot.cpuTemperatureC)} °C`,
    },
    {
      key: "memory",
      label: t("monitoring.memory"),
      value: percent(
        usage(snapshot.memoryPercent, snapshot.memoryUsedBytes, snapshot.memoryTotalBytes),
      ),
      note: `${bytes(snapshot.memoryUsedBytes)} / ${bytes(snapshot.memoryTotalBytes)}`,
    },
    {
      key: "storage",
      label: t("monitoring.storage"),
      value: percent(
        fullest ? usage(fullest.percent, fullest.usedBytes, fullest.totalBytes) : null,
      ),
      note: fullest ? `${bytes(fullest.usedBytes)} / ${bytes(fullest.totalBytes)}` : null,
    },
    {
      key: "network",
      label: t("monitoring.network"),
      value: byteRate(snapshot.networkDownloadBytesPerSecond),
      note: `↑ ${byteRate(snapshot.networkUploadBytesPerSecond)}`,
    },
  ];

  const downloadReading = hasDownloads ? (
    <div className="reading" key="downloads">
      <span className="reading__label">{t("downloadCenter.title")}</span>
      <strong className="reading__value">{String(active.length)}</strong>
      {active.length > 0 ? (
        <span className="reading__note">{byteRate(downloadSpeed)}</span>
      ) : null}
    </div>
  ) : null;

  return (
    <section
      className={monitor.isStale ? "activity activity--stale" : "activity"}
      aria-label={t("shell.activity")}
    >
      <div className="activity__row">
        {readings.map((reading) => (
          <div className="reading" key={reading.key}>
            <span className="reading__label">{reading.label}</span>
            <strong className="reading__value">{reading.value}</strong>
            {reading.note ? <span className="reading__note">{reading.note}</span> : null}
          </div>
        ))}

        {downloadReading}

        {disks.length > 1 ? (
          <button
            type="button"
            className="activity__more"
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
          >
            {t(
              disks.length === 1
                ? "monitoring.volumeCountOne"
                : "monitoring.volumeCountOther",
              { count: String(disks.length) },
            )}
            <ChevronIcon width={12} height={12} />
          </button>
        ) : null}
      </div>

      {open && disks.length > 1 ? (
        <ul className="volumes">
          {disks.map((disk) => {
            const value = usage(disk.percent, disk.usedBytes, disk.totalBytes);
            return (
              <li className="volume" key={disk.mountPoint || disk.name}>
                <span className="volume__name" title={disk.mountPoint || disk.name}>
                  {disk.name}
                </span>
                <span className="volume__track">
                  <span style={{ width: `${value ?? 0}%` }} />
                </span>
                <span className="volume__value">{percent(value)}</span>
                <span className="volume__note">
                  {bytes(disk.usedBytes)} / {bytes(disk.totalBytes)}
                </span>
              </li>
            );
          })}
        </ul>
      ) : null}
    </section>
  );
}
