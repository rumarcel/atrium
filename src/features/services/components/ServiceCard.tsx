import {
  ArrowUpRightIcon,
} from "../../../components/icons/AppIcons";
import type { ServiceHealth } from "../../health/health.types";
import type { DashboardService } from "../service.types";
import type { ServiceContextMenuState } from "./ServiceContextMenu";
import { ServiceIcon } from "./ServiceIcon";

interface ServiceCardProps {
  service: DashboardService;
  health?: ServiceHealth;
  onOpen: (service: DashboardService, openInNewTab: boolean) => void;
  onContextMenu: (state: ServiceContextMenuState) => void;
}

function formatLatency(latencyMs: number): string {
  return latencyMs < 1 ? "<1 ms" : `${latencyMs} ms`;
}

function getHealthPresentation(health?: ServiceHealth) {
  if (!health || health.status === "unchecked") {
    return {
      state: health?.isChecking ? "checking" : "unchecked",
      label: health?.isChecking ? "Checking" : "Not checked",
    };
  }

  if (health.status === "online") {
    return {
      state: "online",
      label: `Online · ${formatLatency(health.latencyMs)}`,
    };
  }

  if (health.status === "warning") {
    if (health.reason === "tls-exception") {
      return {
        state: "warning",
        label: `Local TLS · ${formatLatency(health.latencyMs)}`,
      };
    }

    if (health.reason === "tls") {
      return { state: "warning", label: "TLS issue" };
    }

    if (health.reason === "runtime") {
      return { state: "warning", label: "Desktop only" };
    }

    return { state: "warning", label: "Attention" };
  }

  if (health.reason === "timeout") {
    return { state: "offline", label: "Timed out" };
  }

  if (health.statusCode !== null) {
    return { state: "offline", label: `HTTP ${health.statusCode}` };
  }

  return { state: "offline", label: "Offline" };
}

export function ServiceCard({
  service,
  health,
  onOpen,
  onContextMenu,
}: ServiceCardProps) {
  const healthPresentation = getHealthPresentation(health);
  const healthDetails = [
    healthPresentation.label,
    health?.message,
    health?.isChecking && health.status !== "unchecked"
      ? "Refreshing status…"
      : null,
  ]
    .filter(Boolean)
    .join("\n");

  return (
    <article
      className="service-card"
      aria-label={`${service.name}, ${healthPresentation.label}`}
      title={`${service.name}\n${service.url}\n${healthDetails}`}
    >
      <button
        className="service-card__action"
        type="button"
        aria-label={`Open ${service.name} in Personal Hub`}
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
        <div>
          <p className="service-card__category">{service.category}</p>
          <h3>{service.name}</h3>
          <p className="service-card__description">{service.description}</p>
        </div>
        <span className="service-card__launch" aria-hidden="true">
          <ArrowUpRightIcon width={18} height={18} />
        </span>
      </div>
    </article>
  );
}
