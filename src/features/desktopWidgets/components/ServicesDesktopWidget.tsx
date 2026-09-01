import type {
  DesktopWidgetMonitor,
  DesktopWidgetService,
} from "../desktopWidget.types";
import { useTranslation } from "../../i18n";
import {
  WidgetAlertIcon,
  WidgetCheckIcon,
  WidgetServicesIcon,
} from "./DesktopWidgetIcons";
import { DesktopWidgetShell } from "./DesktopWidgetShell";

interface ServicesDesktopWidgetProps {
  monitor: DesktopWidgetMonitor;
}

function ServiceAlertRow({ service }: { service: DesktopWidgetService }) {
  const { t } = useTranslation();
  return (
    <article
      className={`desktop-widget-service-alert desktop-widget-service-alert--${service.status}`}
    >
      <span className="desktop-widget-service-alert__icon" aria-hidden="true">
        <WidgetAlertIcon />
      </span>
      <div>
        <strong>{service.name}</strong>
        <small>
          {service.message ??
            (service.status === "offline"
              ? t("widget.serviceOfflineDescription")
              : t("widget.serviceWarningDescription"))}
        </small>
      </div>
      <span className="desktop-widget-service-alert__status">
        {service.status === "offline"
          ? t("service.offline")
          : t("widget.warning")}
      </span>
    </article>
  );
}

export function ServicesDesktopWidget({ monitor }: ServicesDesktopWidgetProps) {
  const { locale, number, t } = useTranslation();
  const snapshot = monitor.snapshot;
  const alerts = (snapshot?.services ?? [])
    .filter((service) => service.status !== "online")
    .sort((left, right) => {
      if (left.status === right.status) {
        return left.name.localeCompare(right.name, locale);
      }

      return left.status === "offline" ? -1 : 1;
    });
  const offlineCount = alerts.filter(
    (service) => service.status === "offline",
  ).length;

  return (
    <DesktopWidgetShell
      kind="services"
      title={t("widget.serviceAttention")}
      subtitle={`${snapshot?.serverName ?? t("widget.homeServer")} · ${
        snapshot?.serverAddress ?? "192.168.1.10"
      }`}
      monitor={monitor}
      icon={<WidgetServicesIcon />}
    >
      <div className="desktop-widget-services-summary">
        <div>
          <strong>{snapshot?.status === "online" ? alerts.length : "—"}</strong>
          <span>
            {snapshot?.status === "online"
              ? t(
                  alerts.length === 1
                    ? "widget.serviceNeedsAttentionOne"
                    : "widget.serviceNeedsAttentionOther",
                  { count: number(alerts.length) },
                )
              : t("widget.attentionPending")}
          </span>
        </div>
        {offlineCount > 0 ? (
          <span className="desktop-widget-services-summary__offline">
            {t("widget.offlineCount", { count: number(offlineCount) })}
          </span>
        ) : null}
      </div>

      <div className="desktop-widget-service-list">
        {alerts.length > 0 ? (
          alerts.map((service) => (
            <ServiceAlertRow key={service.id} service={service} />
          ))
        ) : snapshot?.status === "online" ? (
          <div className="desktop-widget-empty desktop-widget-empty--healthy">
            <WidgetCheckIcon />
            <strong>{t("widget.allServicesHealthy")}</strong>
            <span>{t("widget.allServicesHealthyDescription")}</span>
          </div>
        ) : (
          <div className="desktop-widget-empty">
            <WidgetAlertIcon />
            <strong>{t("widget.serviceStatusUnavailable")}</strong>
            <span>{t("widget.serviceStatusUnavailableDescription")}</span>
          </div>
        )}
      </div>
    </DesktopWidgetShell>
  );
}
