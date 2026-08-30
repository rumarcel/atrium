import type { ServiceHealthById } from "../../health/health.types";
import type { DashboardService } from "../service.types";
import { ServiceCard } from "./ServiceCard";
import type { ServiceContextMenuState } from "./ServiceContextMenu";

interface ServiceGridProps {
  services: readonly DashboardService[];
  isLoading?: boolean;
  emptyTitle?: string;
  emptyDescription?: string;
  healthById?: ServiceHealthById;
  onOpenService: (service: DashboardService, openInNewTab: boolean) => void;
  onOpenContextMenu: (state: ServiceContextMenuState) => void;
}

export function ServiceGrid({
  services,
  isLoading = false,
  emptyTitle = "No services found",
  emptyDescription = "Try another name or category.",
  healthById = {},
  onOpenService,
  onOpenContextMenu,
}: ServiceGridProps) {
  if (isLoading) {
    return (
      <div
        className="service-grid"
        aria-label="Loading service configuration"
        aria-busy="true"
      >
        {Array.from({ length: 8 }, (_, index) => (
          <div
            className="service-card service-card--skeleton"
            aria-hidden="true"
            key={`service-skeleton-${index}`}
          >
            <span className="skeleton-block skeleton-block--icon" />
            <div className="service-card__body">
              <div>
                <span className="skeleton-block skeleton-block--label" />
                <span className="skeleton-block skeleton-block--title" />
                <span className="skeleton-block skeleton-block--description" />
              </div>
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (services.length === 0) {
    return (
      <div className="service-grid__empty">
        <p>{emptyTitle}</p>
        <span>{emptyDescription}</span>
      </div>
    );
  }

  return (
    <div className="service-grid">
      {services.map((service) => (
        <ServiceCard
          key={service.id}
          service={service}
          health={healthById[service.id]}
          onOpen={onOpenService}
          onContextMenu={onOpenContextMenu}
        />
      ))}
    </div>
  );
}
