import type {
  DesktopWidgetMonitor,
  DesktopWidgetService,
} from "../desktopWidget.types";
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
              ? "The service is offline."
              : "The service needs attention.")}
        </small>
      </div>
      <span className="desktop-widget-service-alert__status">
        {service.status === "offline" ? "Offline" : "Warning"}
      </span>
    </article>
  );
}

export function ServicesDesktopWidget({ monitor }: ServicesDesktopWidgetProps) {
  const snapshot = monitor.snapshot;
  const alerts = (snapshot?.services ?? [])
    .filter((service) => service.status !== "online")
    .sort((left, right) => {
      if (left.status === right.status) {
        return left.name.localeCompare(right.name);
      }

      return left.status === "offline" ? -1 : 1;
    });
  const offlineCount = alerts.filter(
    (service) => service.status === "offline",
  ).length;

  return (
    <DesktopWidgetShell
      kind="services"
      title="Service attention"
      subtitle={`${snapshot?.serverName ?? "Home Server"} · ${
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
              ? `${alerts.length === 1 ? "service needs" : "services need"} attention`
              : "attention status pending"}
          </span>
        </div>
        {offlineCount > 0 ? (
          <span className="desktop-widget-services-summary__offline">
            {offlineCount} offline
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
            <strong>All services healthy</strong>
            <span>There are no server service alerts.</span>
          </div>
        ) : (
          <div className="desktop-widget-empty">
            <WidgetAlertIcon />
            <strong>Service status unavailable</strong>
            <span>Waiting for health checks from the server.</span>
          </div>
        )}
      </div>
    </DesktopWidgetShell>
  );
}
