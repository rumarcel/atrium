import { useTranslation } from "../features/i18n";
import type { DiskMetrics, ServerMetricsMonitor } from "../features/monitoring/monitoring.types";

interface StorageViewProps {
  monitor: ServerMetricsMonitor;
}

/** The level the native notification runtime already alerts on. */
const FULL_AT = 90;

function usage(disk: DiskMetrics): number {
  if (disk.percent !== null && Number.isFinite(disk.percent)) {
    return Math.max(0, Math.min(100, disk.percent));
  }
  return disk.totalBytes > 0
    ? Math.max(0, Math.min(100, (disk.usedBytes / disk.totalBytes) * 100))
    : 0;
}

/**
 * Everything the server actually reports about its storage.
 *
 * Capacity is summed per device rather than per mount, because one disk
 * mounted at several paths is still one disk: adding its size once per mount
 * point would inflate the total by however many bind mounts happen to exist.
 */
export function StorageView({ monitor }: StorageViewProps) {
  const { bytes, number, percent, t } = useTranslation();
  const disks = monitor.snapshot?.disks ?? [];

  if (disks.length === 0) {
    return (
      <>
        <header className="hero">
          <h1>{t("nav.storage")}</h1>
          <p>{t("storage.unavailable")}</p>
        </header>
      </>
    );
  }

  const byDevice = new Map<string, DiskMetrics>();
  for (const disk of disks) {
    const existing = byDevice.get(disk.name);
    if (!existing || disk.totalBytes > existing.totalBytes) {
      byDevice.set(disk.name, disk);
    }
  }
  const devices = [...byDevice.values()];
  const total = devices.reduce((sum, disk) => sum + disk.totalBytes, 0);
  const used = devices.reduce((sum, disk) => sum + disk.usedBytes, 0);
  const free = Math.max(0, total - used);
  const usedShare = total > 0 ? (used / total) * 100 : 0;

  const summary = [
    { key: "total", label: t("storage.totalLabel"), value: bytes(total), note: t("storage.totalNote") },
    {
      key: "used",
      label: t("storage.usedLabel"),
      value: bytes(used),
      note: t("storage.share", { percent: percent(usedShare) }),
    },
    {
      key: "free",
      label: t("storage.freeLabel"),
      value: bytes(free),
      note: t("storage.share", { percent: percent(100 - usedShare) }),
    },
    {
      key: "devices",
      label: t("storage.devicesLabel"),
      value: number(devices.length),
      note: t("storage.mountCount", { count: number(disks.length) }),
    },
  ];

  return (
    <>
      <header className="hero">
        <h1>{t("nav.storage")}</h1>
        <p>{t("storage.subtitle")}</p>
      </header>

      <div className="stat-row">
        {summary.map((item) => (
          <div className="stat" key={item.key}>
            <span className="stat__label">{item.label}</span>
            <strong className="stat__value">{item.value}</strong>
            <span className="stat__note">{item.note}</span>
          </div>
        ))}
      </div>

      <section className="section">
        <div className="section__head">
          <h2>{t("storage.devicesHeading")}</h2>
        </div>

        <div className="cards">
          {devices.map((disk) => {
            const value = usage(disk);
            const full = value >= FULL_AT;
            return (
              <div className="card" key={disk.name}>
                <span className="card__body">
                  <span className="card__title">
                    <span>{disk.name}</span>
                    {disk.fileSystem ? (
                      <span className="pill pill--idle">{disk.fileSystem}</span>
                    ) : null}
                    {full ? (
                      <span className="pill pill--warn">{t("storage.nearlyFull")}</span>
                    ) : null}
                  </span>
                  <span className="meter">
                    <span className="meter__track">
                      <span
                        className={full ? "is-full" : undefined}
                        style={{ width: `${value}%` }}
                      />
                    </span>
                    <span className="meter__value">{percent(value)}</span>
                  </span>
                  <span className="card__note">
                    {bytes(disk.usedBytes)} / {bytes(disk.totalBytes)} · {disk.mountPoint}
                  </span>
                </span>
              </div>
            );
          })}
        </div>
      </section>

      <section className="section">
        <div className="section__head">
          <h2>{t("storage.mountsHeading")}</h2>
        </div>

        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("storage.colDevice")}</th>
                <th>{t("storage.colFileSystem")}</th>
                <th>{t("storage.colMount")}</th>
                <th className="table__num">{t("storage.colSize")}</th>
                <th className="table__num">{t("storage.colUsed")}</th>
              </tr>
            </thead>
            <tbody>
              {disks.map((disk) => {
                const value = usage(disk);
                return (
                  <tr key={`${disk.name}:${disk.mountPoint}`}>
                    <td>{disk.name}</td>
                    <td>{disk.fileSystem ?? "—"}</td>
                    <td className="table__path" title={disk.mountPoint}>
                      {disk.mountPoint}
                    </td>
                    <td className="table__num">{bytes(disk.totalBytes)}</td>
                    <td className="table__num">
                      <span className={value >= FULL_AT ? "is-warning" : undefined}>
                        {percent(value)}
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>
    </>
  );
}
