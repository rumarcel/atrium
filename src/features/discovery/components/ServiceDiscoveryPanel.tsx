import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "../../i18n";
import { ServiceIcon } from "../../services/components/ServiceIcon";
import type { DashboardService } from "../../services/service.types";
import {
  describeDiscoveryError,
  nativeServiceDiscoveryClient,
} from "../discoveryClient";
import { createDiscoveryReviewItems, selectedDiscoveryServices } from "../discoveryModel";
import type {
  ServiceDiscoveryClient,
  ServiceDiscoveryResponse,
  ServerInventoryDiscoveryResponse,
} from "../discovery.types";

const SERVER_AGENT_SOURCE = "server-agent";
const homarrSourceKey = (id: string) => `homarr:${id}`;

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
  const [sourceKey, setSourceKey] = useState(
    sources[0] ? homarrSourceKey(sources[0].id) : SERVER_AGENT_SOURCE,
  );
  const isServerAgent = sourceKey === SERVER_AGENT_SOURCE;
  const [result, setResult] = useState<ServiceDiscoveryResponse | null>(null);
  const [inventory, setInventory] = useState<ServerInventoryDiscoveryResponse | null>(null);
  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  const [isScanning, setIsScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const scanGeneration = useRef(0);
  const context = { persistedServices, existingServices, disabled, client, sourceKey };
  const latestContext = useRef(context);
  latestContext.current = context;

  useEffect(() => {
    if (!isServerAgent && !sources.some((source) => homarrSourceKey(source.id) === sourceKey)) {
      setSourceKey(sources[0] ? homarrSourceKey(sources[0].id) : SERVER_AGENT_SOURCE);
    }
  }, [isServerAgent, sourceKey, sources]);

  useEffect(() => {
    // A scan belongs to the exact saved catalog, draft and source it started with.
    // A late reply must not replace a newer scan or repopulate a changed draft.
    scanGeneration.current += 1;
    setIsScanning(false);
    setResult(null);
    setInventory(null);
    setSelection(new Set());
    setError(null);
    return () => { scanGeneration.current += 1; };
  }, [persistedServices, existingServices, disabled, client, sourceKey]);

  const reviewItems = useMemo(
    () =>
      createDiscoveryReviewItems(result?.candidates ?? [], existingServices, result?.source),
    [existingServices, result?.candidates, result?.source],
  );
  const selectedCount = reviewItems.filter(
    (item) => selection.has(item.key) && item.service !== null && item.duplicateOf === null,
  ).length;

  const scan = async () => {
    if (disabled || isScanning || (!isServerAgent && !sources.some((source) => homarrSourceKey(source.id) === sourceKey))) {
      return;
    }
    const generation = ++scanGeneration.current;
    const isCurrent = () => {
      const current = latestContext.current;
      return generation === scanGeneration.current &&
        current.persistedServices === persistedServices &&
        current.existingServices === existingServices &&
        current.disabled === disabled && current.client === client && current.sourceKey === sourceKey;
    };
    setIsScanning(true);
    setError(null);
    setResult(null);
    setInventory(null);
    setSelection(new Set());
    try {
      const nextInventory = isServerAgent ? await client.discoverServerInventory() : null;
      const next = nextInventory?.discovery ?? await client.discoverHomarrServices(sourceKey.slice("homarr:".length));
      if (!isCurrent()) return;
      setResult(next);
      setInventory(nextInventory);
      const initial = isServerAgent ? [] : createDiscoveryReviewItems(
        next.candidates,
        existingServices,
        next.source,
      )
        .filter((item) => item.service !== null && item.duplicateOf === null)
        .map((item) => item.key);
      setSelection(new Set(initial));
    } catch (reason) {
      if (!isCurrent()) return;
      // Agent responses are never rendered as raw errors: they can contain
      // network diagnostics from an authenticated privileged endpoint.
      setError(isServerAgent ? "server-agent" : describeDiscoveryError(reason));
    } finally {
      if (isCurrent()) setIsScanning(false);
    }
  };

  const addSelected = () => {
    if (disabled || result === null) return;
    const selected = selectedDiscoveryServices(result.candidates, existingServices, selection, result.source);
    if (selected.length === 0) {
      return;
    }
    onAdd(selected);
    setResult(null);
    setInventory(null);
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
        <div className="discovery-toolbar">
          <label className="settings-field">
            <span>{t("discovery.source")}</span>
            <select
              value={sourceKey}
              disabled={disabled}
              onChange={(event) => {
                scanGeneration.current += 1;
                setSourceKey(event.currentTarget.value);
                setIsScanning(false);
                setResult(null);
                setInventory(null);
                setSelection(new Set());
                setError(null);
              }}
            >
              {sources.map((source) => (
                <option key={source.id} value={homarrSourceKey(source.id)}>
                  Homarr — {source.name}
                </option>
              ))}
              <option value={SERVER_AGENT_SOURCE}>{t("discovery.agent.source")}</option>
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

      {isServerAgent ? (
        <div className="settings-inline-note">{t("discovery.agent.setupHelp")}</div>
      ) : null}
      {sources.length === 0 ? <p className="discovery-hint">{t("discovery.noHomarrSource")}</p> : null}
      {disabled ? (
        <p className="discovery-hint">{t("discovery.saveBeforeScan")}</p>
      ) : null}
      {error ? <div className="discovery-error" role="alert">{isServerAgent ? t("discovery.agent.error") : error}</div> : null}

      {inventory ? (
        <div className="settings-inline-note" role="status">
          <p>{t("discovery.agent.ready", {
            count: inventory.sources.filter((source) => source.state === "ready").length,
          })}</p>
          {inventory.sources.map((source) => (
            <p key={source.runtime}>
              <strong>{source.runtime === "docker" ? "Docker" : "Podman"} (rootful): </strong>
              {t(`discovery.agent.state.${source.state}`)}
              {source.ageSeconds !== null ? ` · ${t("discovery.agent.age", { seconds: source.ageSeconds })}` : ""}
              {` · ${t("discovery.agent.containers", { count: source.containerCount })}`}
              {source.skippedCount > 0 ? ` · ${t("discovery.skipped", { count: source.skippedCount })}` : ""}
            </p>
          ))}
          <p>{t(inventory.maintenance.rebootRequired === true
            ? "discovery.agent.rebootRequired" : "discovery.agent.maintenanceUnknown")}</p>
          <p>{t("discovery.agent.urlHint")}</p>
        </div>
      ) : null}

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
            <div className="settings-inline-note">{t(isServerAgent ? "discovery.agent.empty" : "discovery.empty")}</div>
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
                      checked={!unavailable && !duplicate && selection.has(item.key)}
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
                      <span>{item.candidate.url ?? t(isServerAgent ? "discovery.agent.missingUrl" : "discovery.missingUrl")}</span>
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
            <span>{t(isServerAgent ? "discovery.agent.reviewHelp" : "discovery.reviewHelp")}</span>
            <button
              className="settings-button--primary"
              type="button"
              disabled={disabled || selectedCount === 0}
              onClick={addSelected}
            >
              {t("discovery.addSelected", { count: selectedCount })}
            </button>
          </div>
        </div>
      ) : null}

      <p className="discovery-safety-note">{t("discovery.agent.scopeHelp")}</p>
    </section>
  );
}
