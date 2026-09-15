import {
  ArrowUpRightIcon,
} from "../../../components/icons/AppIcons";
import type { ServiceHealth } from "../../health/health.types";
import { useTranslation, type Translator } from "../../i18n";
import type { ServiceDisplayMode } from "../serviceDisplayPreferences";
import type { DashboardService } from "../service.types";
import type { ServiceContextMenuState } from "./ServiceContextMenu";
import { ServiceIcon } from "./ServiceIcon";

interface ServiceCardProps {
  service: DashboardService;
  health?: ServiceHealth;
  displayMode?: ServiceDisplayMode;
  onOpen: (service: DashboardService, openInNewTab: boolean) => void;
  onContextMenu: (state: ServiceContextMenuState) => void;
}

function formatLatency(
  latencyMs: number,
  number: (value: number) => string,
): string {
  return latencyMs < 1 ? "<1 ms" : `${number(latencyMs)} ms`;
}

function getHealthPresentation(
  t: Translator,
  number: (value: number) => string,
  health?: ServiceHealth,
) {
  if (!health || health.status === "unchecked") {
    return {
      state: health?.isChecking ? "checking" : "unchecked",
      label: health?.isChecking
        ? t("service.checking")
        : t("service.notChecked"),
    };
  }

  if (health.status === "online") {
    return {
      state: "online",
      label: t("service.online"),
      latency: formatLatency(health.latencyMs, number),
    };
  }

  if (health.status === "warning") {
    if (health.reason === "tls-exception") {
      return {
        state: "warning",
        label: t("service.localTls"),
        latency: formatLatency(health.latencyMs, number),
      };
    }

    if (health.reason === "tls") {
      return { state: "warning", label: t("service.tlsIssue") };
    }

    if (health.reason === "runtime") {
      return { state: "warning", label: t("service.desktopOnly") };
    }

    return { state: "warning", label: t("service.attention") };
  }

  if (health.reason === "timeout") {
    return { state: "offline", label: t("service.timedOut") };
  }

  if (health.statusCode !== null) {
    return {
      state: "offline",
      label: t("service.httpStatus", { status: number(health.statusCode) }),
    };
  }

  return { state: "offline", label: t("service.offline") };
}

export function ServiceCard({
  service,
  health,
  displayMode = "cards",
  onOpen,
  onContextMenu,
}: ServiceCardProps) {
  const { number, t } = useTranslation();
  const healthPresentation = getHealthPresentation(t, number, health);
  const healthDetails = [
    healthPresentation.label,
    "latency" in healthPresentation ? healthPresentation.latency : null,
    health?.message,
    health?.isChecking && health.status !== "unchecked"
      ? t("service.refreshingStatus")
      : null,
  ]
    .filter(Boolean)
    .join("\n");

  return (
    <article
      className={`service-card service-card--${displayMode}`}
      aria-label={t("service.cardLabel", {
        serviceName: service.name,
        status: healthPresentation.label,
      })}
      title={`${service.name}\n${service.url}\n${healthDetails}`}
    >
      <button
        className="service-card__action"
        type="button"
        aria-label={t("service.openInHub", { serviceName: service.name })}
        onClick={(event) => onOpen(service, event.ctrlKey || event.metaKey)}
        onContextMenu={(event) => {
          event.preventDefault();
          onContextMenu({
            service,
            x: event.clientX,
            y: event.clientY,
            trigger: event.currentTarget,
          });
        }}
        onKeyDown={(event) => {
          if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) {
            return;
          }

          event.preventDefault();
          const rectangle = event.currentTarget.getBoundingClientRect();
          onContextMenu({
            service,
            x: rectangle.left + Math.min(52, rectangle.width / 2),
            y: rectangle.top + Math.min(52, rectangle.height / 2),
            trigger: event.currentTarget,
          });
        }}
      />
      <div className="service-card__topline">
        <div className={`service-icon service-icon--${service.accent}`}>
          <ServiceIcon name={service.icon} />
        </div>
        <div
          className={`service-status service-status--${healthPresentation.state}`}
          title={healthDetails}
        >
          <span className="service-status__dot" aria-hidden="true" />
          <span>{healthPresentation.label}</span>
        </div>
      </div>

      <div className="service-card__body">
        <h3>{service.name}</h3>
        <span className="service-card__launch" aria-hidden="true">
          <ArrowUpRightIcon width={18} height={18} />
        </span>
      </div>
    </article>
  );
}
