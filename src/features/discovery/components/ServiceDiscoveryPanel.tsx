import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "../../i18n";
import { ServiceIcon } from "../../services/components/ServiceIcon";
import type { DashboardService } from "../../services/service.types";
import {
  describeDiscoveryError,
  nativeServiceDiscoveryClient,
} from "../discoveryClient";
import { createDiscoveryReviewItems } from "../discoveryModel";
import type { ServiceDiscoveryClient } from "../discovery.types";

interface ServiceDiscoveryPanelProps {
  persistedServices: readonly DashboardService[];
  existingServices: readonly DashboardService[];
  disabled?: boolean;
  client?: ServiceDiscoveryClient;
  onAdd: (services: readonly DashboardService[]) => void;
}
export function ServiceDiscoveryPanel({
  persistedServices,
  existingServices,
  disabled = false,
  client = nativeServiceDiscoveryClient,
  onAdd,
}: ServiceDiscoveryPanelProps) {
  const { t } = useTranslation();
  const sources = useMemo(
    () =>
      persistedServices.filter(
        (service) =>
          service.enabled && service.authentication.api === "homarr-api-key",
      ),
    [persistedServices],
  );
  const [sourceServiceId, setSourceServiceId] = useState(sources[0]?.id ?? "");
  const [result, setResult] = useState<
    Awaited<ReturnType<ServiceDiscoveryClient["discoverHomarrServices"]>> | null
  >(null);
  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  const [isScanning, setIsScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!sources.some((source) => source.id === sourceServiceId)) {
      setSourceServiceId(sources[0]?.id ?? "");
      setResult(null);
      setSelection(new Set());
    }
  }, [sourceServiceId, sources]);

  const reviewItems = useMemo(
    () =>
      createDiscoveryReviewItems(result?.candidates ?? [], existingServices),
    [existingServices, result?.candidates],
  );

  const scan = async () => {
    if (!sourceServiceId || disabled || isScanning) {
      return;
    }
    setIsScanning(true);
    setError(null);
    setResult(null);
    setSelection(new Set());
    try {
      const next = await client.discoverHomarrServices(sourceServiceId);
      setResult(next);
      const initial = createDiscoveryReviewItems(
        next.candidates,
        existingServices,
      )
        .filter((item) => item.service !== null && item.duplicateOf === null)
        .map((item) => item.key);
      setSelection(new Set(initial));
    } catch (reason) {
      setError(describeDiscoveryError(reason));
    } finally {
      setIsScanning(false);
    }
  };

  const addSelected = () => {
    const selected = reviewItems
      .filter((item) => selection.has(item.key))
      .flatMap((item) => (item.service === null ? [] : [item.service]));
    if (selected.length === 0) {
      return;
    }
    onAdd(selected);
    setResult(null);
    setSelection(new Set());
  };

  return (
    <section className="settings-panel" aria-labelledby="service-discovery-heading">
      <div className="settings-panel__heading">
        <div>
          <p className="settings-kicker">{t("discovery.kicker")}</p>
          <h2 id="service-discovery-heading">{t("discovery.title")}</h2>
        </div>
        <span className="settings-privacy-badge">{t("discovery.readOnly")}</span>
      </div>

      <p className="discovery-description">{t("discovery.description")}</p>
      {sources.length === 0 ? (
        <div className="settings-inline-note">{t("discovery.noHomarrSource")}</div>
      ) : (
        <div className="discovery-toolbar">
          <label className="settings-field">
            <span>{t("discovery.source")}</span>
            <select
              value={sourceServiceId}
              disabled={disabled || isScanning}
              onChange={(event) => {
                setSourceServiceId(event.currentTarget.value);
                setResult(null);
                setSelection(new Set());
                setError(null);
              }}
            >
              {sources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.name}
                </option>
              ))}
            </select>
          </label>
          <button
            className="settings-button--primary"
            type="button"
            disabled={disabled || isScanning}
            onClick={() => void scan()}
          >
            {isScanning ? t("discovery.scanning") : t("discovery.scan")}
          </button>
        </div>
      )}

      {disabled && sources.length > 0 ? (
        <p className="discovery-hint">{t("discovery.saveBeforeScan")}</p>
      ) : null}
      {error ? <div className="discovery-error" role="alert">{error}</div> : null}

      {result ? (
        <div className="discovery-results">
          <div className="discovery-results__summary" role="status">
            <span>
              {t("discovery.found", { count: reviewItems.length })}
            </span>
            {result.skippedCount > 0 ? (
              <span>{t("discovery.skipped", { count: result.skippedCount })}</span>
            ) : null}
          </div>

          {reviewItems.length === 0 ? (
            <div className="settings-inline-note">{t("discovery.empty")}</div>
          ) : (
            <div className="discovery-list">
              {reviewItems.map((item) => {
                const unavailable = item.service === null;
                const duplicate = item.duplicateOf !== null;
                return (
                  <label
                    className={`discovery-item${
                      unavailable || duplicate ? " discovery-item--unavailable" : ""
                    }`}
                    key={item.key}
                  >
                    <input
                      type="checkbox"
                      checked={selection.has(item.key)}
                      disabled={disabled || unavailable || duplicate}
                      onChange={(event) => {
                        const checked = event.currentTarget.checked;
                        setSelection((current) => {
                          const next = new Set(current);
                          if (checked) {
                            next.add(item.key);
                          } else {
                            next.delete(item.key);
                          }
                          return next;
                        });
                      }}
                    />
                    <span
                      className={`discovery-item__icon service-icon service-icon--${
                        item.service?.accent ?? "slate"
                      }`}
                      aria-hidden="true"
                    >
                      <ServiceIcon name={item.service?.icon ?? "service"} />
                    </span>
                    <span className="discovery-item__copy">
                      <strong>{item.candidate.name}</strong>
                      <span>{item.candidate.url ?? t("discovery.missingUrl")}</span>
                    </span>
                    <span className="discovery-item__meta">
                      {duplicate
                        ? t("discovery.duplicate", { serviceName: item.duplicateOf ?? "" })
                        : unavailable
                          ? t("discovery.reviewRequired")
                          : t(`discovery.confidence.${item.iconConfidence}`)}
                    </span>
                  </label>
                );
              })}
            </div>
          )}

          <div className="discovery-actions">
            <span>{t("discovery.reviewHelp")}</span>
            <button
              className="settings-button--primary"
              type="button"
              disabled={disabled || selection.size === 0}
              onClick={addSelected}
            >
              {t("discovery.addSelected", { count: selection.size })}
            </button>
          </div>
        </div>
      ) : null}

      <p className="discovery-safety-note">{t("discovery.containerDeferred")}</p>
    </section>
  );
}
