import { useId, useMemo } from "react";
import {
  useTranslation,
  type TranslationKeysWithoutParameters,
} from "../../i18n";
import { ServiceIcon } from "../../services/components/ServiceIcon";
import type { DashboardService } from "../../services/service.types";
import type {
  DownloadCenterUnavailableReason,
  DownloadItem,
  DownloadItemState,
} from "../downloads.types";
import { useDownloadCenter } from "../hooks/useDownloadCenter";

interface DownloadCenterProps {
  providers: readonly DashboardService[];
  enabled?: boolean;
  onOpenSettings: (serviceId: string | null) => void;
}

const STATE_LABELS: Readonly<
  Record<DownloadItemState, TranslationKeysWithoutParameters>
> = {
  downloading: "downloadCenter.state.downloading",
  queued: "downloadCenter.state.queued",
  stalled: "downloadCenter.state.stalled",
  paused: "downloadCenter.state.paused",
  checking: "downloadCenter.state.checking",
  metadata: "downloadCenter.state.metadata",
  error: "downloadCenter.state.error",
  other: "downloadCenter.state.other",
};

const REASON_LABELS: Readonly<
  Record<
    Exclude<DownloadCenterUnavailableReason, "backoff">,
    TranslationKeysWithoutParameters
  >
> = {
  authentication: "downloadCenter.reason.authentication",
  timeout: "downloadCenter.reason.timeout",
  tls: "downloadCenter.reason.tls",
  connection: "downloadCenter.reason.connection",
  "api-unavailable": "downloadCenter.reason.apiUnavailable",
  "invalid-data": "downloadCenter.reason.invalidData",
};

function DownloadRow({
  item,
  providerName,
}: {
  item: DownloadItem;
  providerName: string;
}) {
  const { byteRate, duration, percent, t } = useTranslation();
  const progress = Math.max(0, Math.min(100, item.progressPercent));
  const eta =
    item.etaSeconds === null
      ? t("downloadCenter.etaUnavailable")
      : duration(item.etaSeconds, {
          maximumParts: 2,
          smallestUnit: "minute",
          placeholder: t("downloadCenter.etaUnavailable"),
        });
  const details = [
    t(STATE_LABELS[item.state]),
    t("downloadCenter.speed", {
      speed: byteRate(item.downloadSpeedBytesPerSecond),
    }),
    t("downloadCenter.eta", { eta }),
    item.category
      ? t("downloadCenter.category", { category: item.category })
      : null,
    item.tags.length > 0
      ? t("downloadCenter.tags", { tags: item.tags.join(", ") })
      : null,
  ].filter((value): value is string => value !== null);

  return (
    <article className="download-item">
      <div className="download-item__heading">
        <h3 title={item.name}>{item.name}</h3>
        <div className="download-item__badges">
          <span className="download-badge download-badge--provider">
            {providerName}
          </span>
          {item.source ? (
            <span
              className={`download-badge download-badge--source download-badge--${item.source.kind}`}
              title={t("downloadCenter.source", { source: item.source.label })}
            >
              {item.source.label}
            </span>
          ) : null}
        </div>
      </div>

      <div className="download-item__progress-row">
        <div
          className="download-item__progress"
          role="progressbar"
          aria-label={t("downloadCenter.progress", { name: item.name })}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(progress)}
        >
          <span style={{ width: `${progress}%` }} />
        </div>
        <strong>{percent(progress)}</strong>
      </div>

      <p className="download-item__details" title={details.join(" · ")}>
        {details.map((detail, index) => (
          <span key={`${index}:${detail}`}>{detail}</span>
        ))}
      </p>
    </article>
  );
}

export function DownloadCenter({
  providers,
  enabled = true,
  onOpenSettings,
}: DownloadCenterProps) {
  const { byteRate, number, t } = useTranslation();
  const headingId = useId();
  const providerRevision = useMemo(
    () =>
      providers
        .map(
          (provider) =>
            `${provider.id}\u0000${provider.url}\u0000${provider.tlsPolicy}\u0000${provider.authentication.api}\u0000${provider.authentication.browser}\u0000${provider.authentication.allowInsecureLocalHttp}`,
        )
        .join("\u0001"),
    [providers],
  );
  const monitor = useDownloadCenter({
    enabled: enabled && providers.length > 0,
    revision: providerRevision,
  });

  if (providers.length === 0) {
    return null;
  }
  if (
    monitor.result !== null &&
    monitor.result.providerState === "not-configured"
  ) {
    return null;
  }

  const snapshot = monitor.snapshot;
  const provider = monitor.result?.provider ?? snapshot?.provider ?? null;
  const fallbackProvider = providers[0] ?? null;
  const providerName = provider?.name ?? fallbackProvider?.name ?? "qBittorrent";
  const providerServiceId = provider?.serviceId ?? fallbackProvider?.id ?? null;
  const selectedProvider =
    providers.find((service) => service.id === providerServiceId) ?? fallbackProvider;
  const items = snapshot?.items ?? [];
  const reason = monitor.result?.reason ?? null;
  const unavailable = monitor.status === "unavailable";
  const reasonDescription =
    reason === "backoff"
      ? t("downloadCenter.reason.backoff", {
          count: Math.max(
            1,
            Math.ceil((monitor.result?.retryAfterMs ?? 1_000) / 1_000),
          ),
        })
      : reason
        ? t(REASON_LABELS[reason])
        : null;
  const message = reason === null
    ? monitor.result?.message ?? reasonDescription
    : reasonDescription;

  return (
    <section
      className={`download-center${monitor.isStale ? " download-center--stale" : ""}`}
      aria-labelledby={headingId}
      aria-busy={monitor.isRefreshing}
    >
      <header className="download-center__header">
        <div className="download-center__identity">
          <span
            className={`download-center__icon service-icon service-icon--${selectedProvider?.accent ?? "blue"}`}
            aria-hidden="true"
          >
            <ServiceIcon name={selectedProvider?.icon ?? "qbittorrent"} />
          </span>
          <div>
            <div className="download-center__title-row">
              <h2 id={headingId}>{t("downloadCenter.title")}</h2>
              <span className="download-center__read-only">
                {t("downloadCenter.readOnly")}
              </span>
            </div>
            <p>
              {monitor.status === "loading"
                ? t("downloadCenter.loading")
                : t("downloadCenter.summary", {
                    count: number(items.length),
                    speed: byteRate(
                      snapshot?.totalDownloadSpeedBytesPerSecond ?? 0,
                    ),
                  })}
            </p>
          </div>
        </div>
      </header>

      {unavailable ? (
        <div className="download-center__notice" role="status">
          <div>
            <strong>
              {reason === "authentication" || reason === "backoff"
                ? t("downloadCenter.authenticationTitle")
                : t("downloadCenter.unavailableTitle")}
            </strong>
            <span>{message}</span>
          </div>
          <button type="button" onClick={() => onOpenSettings(providerServiceId)}>
            {t("downloadCenter.openSettings")}
          </button>
        </div>
      ) : null}

      {monitor.status === "loading" ? (
        <div className="download-center__loading" role="status">
          <span />
          <span />
          <span />
        </div>
      ) : items.length === 0 && !unavailable ? (
        <p className="download-center__empty" role="status">
          {t("downloadCenter.noPendingTitle")}
        </p>
      ) : items.length > 0 ? (
        <div
          className="download-center__list"
          aria-label={
            monitor.isStale
              ? t("downloadCenter.lastKnownDownloads")
              : t("downloadCenter.pendingDownloads")
          }
        >
          {items.map((item) => (
            <DownloadRow key={item.id} item={item} providerName={providerName} />
          ))}
        </div>
      ) : null}
      {items.length > 0 ? (
        <p className="download-center__limit-note">{t("downloadCenter.limitNote")}</p>
      ) : null}
    </section>
  );
}
