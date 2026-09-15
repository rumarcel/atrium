import {
  ChevronRightIcon,
  DownloadIcon,
  NetworkIcon,
  PlayIcon,
  RefreshIcon,
  ServerIcon,
  StorageIcon,
} from "../components/icons/AppIcons";
import { useTranslation } from "../features/i18n";
import type { ServiceHealthById } from "../features/health/health.types";
import type { ServerMetricsMonitor } from "../features/monitoring/monitoring.types";
import { ServiceIcon } from "../features/services/components/ServiceIcon";
import type { DashboardService } from "../features/services/service.types";
import { serviceState } from "./serviceState";
import type { ShellView } from "./Sidebar";

interface HomeViewProps {
  services: readonly DashboardService[];
  health: ServiceHealthById;
  monitor: ServerMetricsMonitor;
  isLoading: boolean;
  onOpenService: (service: DashboardService) => void;
  onNavigate: (view: ShellView) => void;
  onRefreshAll: () => void;
}

const PILL: Readonly<Record<string, string>> = {
  ok: "pill pill--ok",
  attention: "pill pill--warn",
  down: "pill pill--bad",
  pending: "pill pill--idle",
};

function ratio(
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
 * The landing view: how the server is, the services worth one click, and the
 * two or three things most often done next.
 *
 * Only the first six services appear here. The point of a landing page is to
 * be shorter than the full list, otherwise it is the full list with a heading.
 */
export function HomeView({
  services,
  health,
  monitor,
  isLoading,
  onOpenService,
  onNavigate,
  onRefreshAll,
}: HomeViewProps) {
  const { byteRate, bytes, percent, t, uptime } = useTranslation();
  const snapshot = monitor.snapshot;

  const disks = snapshot?.disks ?? [];
  const fullest = disks.reduce<(typeof disks)[number] | null>(
    (worst, disk) =>
      worst === null ||
      (ratio(disk.percent, disk.usedBytes, disk.totalBytes) ?? -1) >
        (ratio(worst.percent, worst.usedBytes, worst.totalBytes) ?? -1)
        ? disk
        : worst,
    null,
  );
  const diskPercent = fullest
    ? ratio(fullest.percent, fullest.usedBytes, fullest.totalBytes)
    : null;

  const online = monitor.status === "online" && !monitor.isStale;
  const featured = services.slice(0, 6);
  const media = services.find((service) => service.category === "Media") ?? null;
  const downloads =
    services.find(
      (service) => service.authentication.api === "qbittorrent-web-api",
    ) ?? services.find((service) => service.category === "Downloads") ?? null;

  return (
    <>
      <header className="hero">
        <p className="hero__eyebrow">{t("home.greeting")}</p>
        <h1>{t("home.question")}</h1>
        <p>{t("home.subtitle")}</p>
      </header>

      <div className="cards">
        <button type="button" className="card" onClick={() => onNavigate("servers")}>
          <span className="card__icon card__icon--blue" aria-hidden="true">
            <ServerIcon width={24} height={24} />
          </span>
          <span className="card__body">
            <span className="card__title">
              <span>{t("home.serverStatus")}</span>
            </span>
            <span className="card__note">
              <span className={online ? "dot dot--ok" : "dot dot--bad"} aria-hidden="true" />
              {online ? t("verdict.allWell") : t("verdict.unreachable")}
            </span>
            {snapshot?.uptimeSeconds != null ? (
              <span className="card__note">
                {t("home.uptimeLabel", { uptime: uptime(snapshot.uptimeSeconds) })}
              </span>
            ) : null}
          </span>
          <ChevronRightIcon className="card__chevron" width={18} height={18} />
        </button>

        <button type="button" className="card" onClick={() => onNavigate("servers")}>
          <span className="card__icon card__icon--cyan" aria-hidden="true">
            <StorageIcon width={24} height={24} />
          </span>
          <span className="card__body">
            <span className="card__title">
              <span>{t("monitoring.storage")}</span>
            </span>
            <span className="meter">
              <span className="meter__track">
                <span style={{ width: `${diskPercent ?? 0}%` }} />
              </span>
              <span className="meter__value">{percent(diskPercent)}</span>
            </span>
            {fullest ? (
              <span className="card__note">
                {t("home.storageUsed", {
                  used: bytes(fullest.usedBytes),
                  total: bytes(fullest.totalBytes),
                })}
              </span>
            ) : null}
          </span>
          <ChevronRightIcon className="card__chevron" width={18} height={18} />
        </button>

        <button type="button" className="card" onClick={() => onNavigate("servers")}>
          <span className="card__icon card__icon--green" aria-hidden="true">
            <NetworkIcon width={24} height={24} />
          </span>
          <span className="card__body">
            <span className="card__title">
              <span>{t("home.networkLabel")}</span>
            </span>
            {snapshot ? (
              <span className="card__note">
                {t("home.networkRates", {
                  download: byteRate(snapshot.networkDownloadBytesPerSecond),
                  upload: byteRate(snapshot.networkUploadBytesPerSecond),
                })}
              </span>
            ) : (
              <span className="card__note">{t("verdict.unreachable")}</span>
            )}
          </span>
          <ChevronRightIcon className="card__chevron" width={18} height={18} />
        </button>
      </div>

      <section className="section">
        <div className="section__head">
          <h2>{t("home.myServices")}</h2>
          <button
            type="button"
            className="section__link"
            onClick={() => onNavigate("services")}
          >
            {t("home.seeAll")}
            <ChevronRightIcon width={15} height={15} />
          </button>
        </div>

        <div className="cards">
          {isLoading
            ? Array.from({ length: 6 }, (_, index) => (
                <span className="card card--loading" key={index} aria-hidden="true" />
              ))
            : featured.map((service) => {
                const state = serviceState(health[service.id]);
                return (
                  <button
                    type="button"
                    className="card"
                    key={service.id}
                    title={`${service.name}\n${service.url}`}
                    onClick={() => onOpenService(service)}
                  >
                    <span
                      className={`card__icon card__icon--${service.accent}`}
                      aria-hidden="true"
                    >
                      <ServiceIcon name={service.icon} />
                    </span>
                    <span className="card__body">
                      <span className="card__title">
                        <span>{service.name}</span>
                        <span className={PILL[state]}>
                          {t(
                            state === "down"
                              ? "service.offline"
                              : state === "attention"
                                ? "service.attention"
                                : state === "pending"
                                  ? "service.notChecked"
                                  : "service.online",
                          )}
                        </span>
                      </span>
                      <span className="card__note">{service.description}</span>
                    </span>
                    <ChevronRightIcon className="card__chevron" width={18} height={18} />
                  </button>
                );
              })}
        </div>
      </section>

      <section className="section">
        <div className="section__head">
          <h2>{t("home.quickActions")}</h2>
        </div>

        <div className="cards">
          <button
            type="button"
            className="card"
            disabled={media === null}
            onClick={() => media && onOpenService(media)}
          >
            <span className="card__icon card__icon--violet" aria-hidden="true">
              <PlayIcon width={24} height={24} />
            </span>
            <span className="card__body">
              <span className="card__title">
                <span>{t("home.openMedia")}</span>
              </span>
              <span className="card__note">
                {media
                  ? t("home.openMediaNote", { serviceName: media.name })
                  : t("home.noQuickAction")}
              </span>
            </span>
            <ChevronRightIcon className="card__chevron" width={18} height={18} />
          </button>

          <button
            type="button"
            className="card"
            onClick={() => onNavigate("downloads")}
          >
            <span className="card__icon card__icon--blue" aria-hidden="true">
              <DownloadIcon width={24} height={24} />
            </span>
            <span className="card__body">
              <span className="card__title">
                <span>{t("home.openDownloads")}</span>
              </span>
              <span className="card__note">
                {downloads
                  ? t("home.openDownloadsNote", { serviceName: downloads.name })
                  : t("home.noQuickAction")}
              </span>
            </span>
            <ChevronRightIcon className="card__chevron" width={18} height={18} />
          </button>

          <button type="button" className="card" onClick={onRefreshAll}>
            <span className="card__icon card__icon--green" aria-hidden="true">
              <RefreshIcon width={24} height={24} />
            </span>
            <span className="card__body">
              <span className="card__title">
                <span>{t("home.refreshAll")}</span>
              </span>
              <span className="card__note">{t("home.refreshAllNote")}</span>
            </span>
            <ChevronRightIcon className="card__chevron" width={18} height={18} />
          </button>
        </div>
      </section>
    </>
  );
}
