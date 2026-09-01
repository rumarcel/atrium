import {
  useCallback,
  useEffect,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";
import {
  disableDesktopWidget,
  startDesktopWidgetDrag,
} from "../desktopWidgetClient";
import type {
  DesktopWidgetKind,
  DesktopWidgetMonitor,
} from "../desktopWidget.types";
import {
  WidgetCloseIcon,
  WidgetRefreshIcon,
} from "./DesktopWidgetIcons";

interface DesktopWidgetShellProps {
  kind: DesktopWidgetKind;
  title: string;
  subtitle: string;
  monitor: DesktopWidgetMonitor;
  icon: ReactNode;
  children: ReactNode;
}

export function DesktopWidgetShell({
  kind,
  title,
  subtitle,
  monitor,
  icon,
  children,
}: DesktopWidgetShellProps) {
  const [windowActionError, setWindowActionError] = useState<string | null>(null);
  const [isDisabling, setIsDisabling] = useState(false);

  useEffect(() => {
    document.documentElement.classList.add("desktop-widget-document");
    document.body.classList.add("desktop-widget-document");

    return () => {
      document.documentElement.classList.remove("desktop-widget-document");
      document.body.classList.remove("desktop-widget-document");
    };
  }, []);

  const handleDisable = useCallback(async () => {
    setIsDisabling(true);
    setWindowActionError(null);

    try {
      await disableDesktopWidget(kind);
    } catch (error) {
      setWindowActionError(
        error instanceof Error && error.message.trim().length > 0
          ? error.message.slice(0, 240)
          : "This desktop card could not be disabled.",
      );
      setIsDisabling(false);
    }
  }, [kind]);

  const handleDragStart = useCallback((event: MouseEvent<HTMLElement>) => {
    if (
      event.button !== 0 ||
      (event.target instanceof Element && event.target.closest("button") !== null)
    ) {
      return;
    }

    event.preventDefault();
    void startDesktopWidgetDrag().catch((error: unknown) => {
      setWindowActionError(
        error instanceof Error && error.message.trim().length > 0
          ? error.message.slice(0, 240)
          : "This desktop card could not be moved.",
      );
    });
  }, []);

  const monitoringNotConfigured =
    kind !== "services" &&
    monitor.snapshot?.providerState === "not-configured";
  const connectionTone = monitoringNotConfigured
    ? "inactive"
    : monitor.snapshot?.status === "online" && !monitor.isStale
      ? "online"
      : monitor.isLoading
        ? "loading"
        : "unavailable";
  const connectionLabel = (() => {
    if (kind === "services") {
      if (connectionTone === "online") {
        return "Health checks complete";
      }

      return connectionTone === "loading"
        ? "Checking server services"
        : "Service data unavailable";
    }

    if (monitoringNotConfigured) {
      return "Monitoring not configured";
    }

    if (connectionTone === "online") {
      return "Server online";
    }

    return connectionTone === "loading"
      ? "Connecting to server"
      : "Server data unavailable";
  })();

  return (
    <main className={`desktop-widget-root desktop-widget-root--${kind}`}>
      <section
        className={`desktop-widget-card desktop-widget-card--${connectionTone}`}
        aria-label={`${title} desktop card`}
        aria-busy={monitor.isRefreshing}
      >
        <header
          className="desktop-widget-header"
          onMouseDown={handleDragStart}
        >
          <div className="desktop-widget-header__identity">
            <span className="desktop-widget-header__icon" aria-hidden="true">
              {icon}
            </span>
            <span className="desktop-widget-header__copy">
              <strong>{title}</strong>
              <small>{subtitle}</small>
            </span>
          </div>

          <div className="desktop-widget-controls">
            <button
              type="button"
              onClick={monitor.refresh}
              disabled={
                monitor.isRefreshing ||
                monitor.isPaused ||
                monitoringNotConfigured
              }
              aria-label={`Refresh ${title}`}
              title="Refresh server data"
            >
              <WidgetRefreshIcon
                className={monitor.isRefreshing ? "is-spinning" : undefined}
              />
            </button>
            <button
              type="button"
              onClick={handleDisable}
              disabled={isDisabling}
              aria-label={`Turn off ${title}`}
              title="Turn off this desktop card"
            >
              <WidgetCloseIcon />
            </button>
          </div>
        </header>

        <div className="desktop-widget-connection" role="status" aria-live="polite">
          <span
            className={`desktop-widget-status-dot desktop-widget-status-dot--${connectionTone}`}
            aria-hidden="true"
          />
          <span>{connectionLabel}</span>
        </div>

        <div className="desktop-widget-content">{children}</div>

        {monitor.error || windowActionError ? (
          <p className="desktop-widget-message" role="status">
            {windowActionError ?? monitor.error}
          </p>
        ) : null}
      </section>
    </main>
  );
}
