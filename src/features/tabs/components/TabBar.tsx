import { useMemo, useRef, type KeyboardEvent } from "react";
import {
  CloseIcon,
  GridIcon,
} from "../../../components/icons/AppIcons";
import { useTranslation } from "../../i18n";
import { ServiceIcon } from "../../services/components/ServiceIcon";
import type { DashboardService } from "../../services/service.types";
import { DASHBOARD_TAB_ID, type ActiveTabId } from "../tab.types";

interface TabBarProps {
  activeTabId: ActiveTabId;
  services: readonly DashboardService[];
  onActivate: (tabId: ActiveTabId) => void;
  onClose: (serviceId: string) => void;
}

export function TabBar({
  activeTabId,
  services,
  onActivate,
  onClose,
}: TabBarProps) {
  const { t } = useTranslation();
  const tabRefs = useRef(new Map<string, HTMLButtonElement>());
  const orderedTabIds = useMemo(
    () => [DASHBOARD_TAB_ID, ...services.map((service) => service.id)],
    [services],
  );

  const focusTab = (tabId: string) => {
    onActivate(tabId);
    requestAnimationFrame(() => tabRefs.current.get(tabId)?.focus());
  };

  const handleTabKeyDown = (
    event: KeyboardEvent<HTMLButtonElement>,
    tabId: string,
  ) => {
    const currentIndex = orderedTabIds.indexOf(tabId);
    let nextIndex: number | null = null;

    switch (event.key) {
      case "ArrowLeft":
        nextIndex =
          (currentIndex - 1 + orderedTabIds.length) % orderedTabIds.length;
        break;
      case "ArrowRight":
        nextIndex = (currentIndex + 1) % orderedTabIds.length;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = orderedTabIds.length - 1;
        break;
      default:
        return;
    }

    event.preventDefault();
    focusTab(orderedTabIds[nextIndex]);
  };

  const registerTab = (tabId: string, element: HTMLButtonElement | null) => {
    if (element) {
      tabRefs.current.set(tabId, element);
    } else {
      tabRefs.current.delete(tabId);
    }
  };

  return (
    <div className="tab-bar" aria-label={t("tabs.openViews")}>
      <div className="tab-bar__track" role="tablist" aria-label={t("tabs.listLabel")}>
        <button
          ref={(element) => registerTab(DASHBOARD_TAB_ID, element)}
          className={
            activeTabId === DASHBOARD_TAB_ID
              ? "app-tab app-tab--dashboard app-tab--active"
              : "app-tab app-tab--dashboard"
          }
          type="button"
          role="tab"
          aria-selected={activeTabId === DASHBOARD_TAB_ID}
          aria-controls="dashboard-panel"
          tabIndex={activeTabId === DASHBOARD_TAB_ID ? 0 : -1}
          onClick={() => onActivate(DASHBOARD_TAB_ID)}
          onKeyDown={(event) => handleTabKeyDown(event, DASHBOARD_TAB_ID)}
        >
          <GridIcon width={15} height={15} />
          <span>{t("tabs.dashboard")}</span>
        </button>

        {services.map((service) => {
          const isActive = activeTabId === service.id;

          return (
            <div
              className={
                isActive
                  ? "service-tab service-tab--active"
                  : "service-tab"
              }
              key={service.id}
            >
              <button
                ref={(element) => registerTab(service.id, element)}
                className="app-tab app-tab--service"
                type="button"
                role="tab"
                aria-selected={isActive}
                aria-controls="service-webview-panel"
                tabIndex={isActive ? 0 : -1}
                title={`${service.name}\n${service.url}`}
                onClick={() => onActivate(service.id)}
                onKeyDown={(event) => handleTabKeyDown(event, service.id)}
              >
                <span
                  className={`app-tab__service-icon app-tab__service-icon--${service.accent}`}
                  aria-hidden="true"
                >
                  <ServiceIcon name={service.icon} />
                </span>
                <span className="app-tab__label">{service.name}</span>
              </button>
              <button
                className="service-tab__close"
                type="button"
                aria-label={t("tabs.closeTab", { serviceName: service.name })}
                title={t("tabs.closeService", { serviceName: service.name })}
                onClick={(event) => {
                  event.stopPropagation();
                  onClose(service.id);
                }}
              >
                <CloseIcon width={14} height={14} />
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
