import type { ServiceHealthById } from "../../health/health.types";
import { useTranslation } from "../../i18n";
import type { ServiceDisplayMode } from "../serviceDisplayPreferences";
import type { DashboardService } from "../service.types";
import { ServiceCard } from "./ServiceCard";
import type { ServiceContextMenuState } from "./ServiceContextMenu";

interface ServiceGridProps {
  services: readonly DashboardService[];
  isLoading?: boolean;
  emptyTitle?: string;
  emptyDescription?: string;
  healthById?: ServiceHealthById;
  displayMode?: ServiceDisplayMode;
  onOpenService: (service: DashboardService, openInNewTab: boolean) => void;
  onOpenContextMenu: (state: ServiceContextMenuState) => void;
}

export function ServiceGrid({
  services,
  isLoading = false,
  emptyTitle,
  emptyDescription,
  healthById = {},
  displayMode = "cards",
  onOpenService,
  onOpenContextMenu,
}: ServiceGridProps) {
  const { t } = useTranslation();
  const resolvedEmptyTitle = emptyTitle ?? t("dashboard.noServicesFound");
  const resolvedEmptyDescription =
    emptyDescription ?? t("dashboard.noServicesFoundDescription");

  if (isLoading) {
    return (
      <div
        className={`service-grid service-grid--${displayMode}`}
        aria-label={t("service.gridLoading")}
        aria-busy="true"
      >
        {Array.from({ length: 8 }, (_, index) => (
          <div
            className={`service-card service-card--${displayMode} service-card--skeleton`}
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
        <p>{resolvedEmptyTitle}</p>
        <span>{resolvedEmptyDescription}</span>
      </div>
    );
  }

  return (
    <div className={`service-grid service-grid--${displayMode}`}>
      {services.map((service) => (
        <ServiceCard
          key={service.id}
          service={service}
          health={healthById[service.id]}
          displayMode={displayMode}
          onOpen={onOpenService}
          onContextMenu={onOpenContextMenu}
        />
      ))}
    </div>
  );
}
