import type { MouseEvent } from "react";
import { useTranslation } from "../features/i18n";
import type { ServiceHealthById } from "../features/health/health.types";
import { ServiceIcon } from "../features/services/components/ServiceIcon";
import type { DashboardService } from "../features/services/service.types";
import type { ServiceContextMenuState } from "../features/services/components/ServiceContextMenu";
import { serviceState, STATE_DOT } from "./serviceState";

interface ServiceGridProps {
  services: readonly DashboardService[];
  health: ServiceHealthById;
  isLoading: boolean;
  onOpen: (service: DashboardService, inNewTab: boolean) => void;
  onContextMenu: (state: ServiceContextMenuState) => void;
}

/**
 * The services are the page, so they get the space and the plain treatment:
 * a name, a mark, and a dot only when the dot has something to say. A tile is
 * one button, which is what it is — there is nothing inside it to click past.
 */
export function ServiceGrid({
  services,
  health,
  isLoading,
  onOpen,
  onContextMenu,
}: ServiceGridProps) {
  const { t } = useTranslation();

  if (isLoading) {
    return (
      <div className="tiles" aria-busy="true">
        {Array.from({ length: 8 }, (_, index) => (
          <span className="tile tile--loading" key={index} aria-hidden="true" />
        ))}
      </div>
    );
  }

  if (services.length === 0) {
    return (
      <p className="tiles-empty" role="status">
        {t("dashboard.noServices")}
      </p>
    );
  }

  const openMenu = (
    service: DashboardService,
    element: HTMLElement,
    x: number,
    y: number,
  ) => {
    onContextMenu({ service, x, y, trigger: element });
  };

  return (
    <ul className="tiles">
      {services.map((service) => {
        const state = serviceState(health[service.id]);
        const message = health[service.id]?.message ?? null;

        return (
          <li key={service.id}>
            <button
              type="button"
              className={`tile tile--${state}`}
              title={`${service.name}\n${service.url}${message ? `\n${message}` : ""}`}
              aria-label={t("service.openInHub", { serviceName: service.name })}
              onClick={(event: MouseEvent) =>
                onOpen(service, event.ctrlKey || event.metaKey)
              }
              onContextMenu={(event) => {
                event.preventDefault();
                openMenu(service, event.currentTarget, event.clientX, event.clientY);
              }}
              onKeyDown={(event) => {
                if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) {
                  return;
                }
                event.preventDefault();
                const box = event.currentTarget.getBoundingClientRect();
                openMenu(service, event.currentTarget, box.left + 40, box.top + 40);
              }}
            >
              <span className={`tile__mark tile__mark--${service.accent}`} aria-hidden="true">
                <ServiceIcon name={service.icon} />
              </span>
              <span className="tile__name">{service.name}</span>
              {state === "ok" || state === "pending" ? null : (
                <span className={`${STATE_DOT[state]} tile__state`} aria-hidden="true" />
              )}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
