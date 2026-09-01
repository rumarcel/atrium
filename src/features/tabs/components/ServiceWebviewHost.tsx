import { useLayoutEffect, useRef } from "react";
import { useTranslation } from "../../i18n";
import type { ServiceWebviewBounds } from "../nativeServiceWebviews";

interface ServiceWebviewHostProps {
  serviceId: string;
  serviceName: string;
  status: "loading" | "ready" | "error";
  error: string | null;
  onBoundsChange: (
    serviceId: string,
    bounds: ServiceWebviewBounds,
    scaleFactor: number,
  ) => void;
}

interface HostMeasurement {
  bounds: ServiceWebviewBounds;
  scaleFactor: number;
}

function measurementsAreEqual(
  first: HostMeasurement | null,
  second: HostMeasurement,
) {
  return (
    first !== null &&
    Math.abs(first.bounds.x - second.bounds.x) < 0.5 &&
    Math.abs(first.bounds.y - second.bounds.y) < 0.5 &&
    Math.abs(first.bounds.width - second.bounds.width) < 0.5 &&
    Math.abs(first.bounds.height - second.bounds.height) < 0.5 &&
    Math.abs(first.scaleFactor - second.scaleFactor) < 0.001
  );
}

export function ServiceWebviewHost({
  serviceId,
  serviceName,
  status,
  error,
  onBoundsChange,
}: ServiceWebviewHostProps) {
  const { t } = useTranslation();
  const viewportRef = useRef<HTMLDivElement>(null);
  const lastMeasurementRef = useRef<HostMeasurement | null>(null);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;

    if (!viewport) {
      return;
    }

    let animationFrame = 0;

    const measure = () => {
      cancelAnimationFrame(animationFrame);
      animationFrame = requestAnimationFrame(() => {
        const rectangle = viewport.getBoundingClientRect();

        if (rectangle.width < 1 || rectangle.height < 1) {
          return;
        }

        const measurement = {
          bounds: {
            x: rectangle.left,
            y: rectangle.top,
            width: rectangle.width,
            height: rectangle.height,
          },
          scaleFactor: window.devicePixelRatio || 1,
        };

        if (!measurementsAreEqual(lastMeasurementRef.current, measurement)) {
          lastMeasurementRef.current = measurement;
          onBoundsChange(
            serviceId,
            measurement.bounds,
            measurement.scaleFactor,
          );
        }
      });
    };

    let resolutionQuery = window.matchMedia(
      `(resolution: ${window.devicePixelRatio || 1}dppx)`,
    );
    const handleResolutionChange = () => {
      resolutionQuery.removeEventListener("change", handleResolutionChange);
      resolutionQuery = window.matchMedia(
        `(resolution: ${window.devicePixelRatio || 1}dppx)`,
      );
      resolutionQuery.addEventListener("change", handleResolutionChange);
      measure();
    };

    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    resolutionQuery.addEventListener("change", handleResolutionChange);
    window.addEventListener("resize", measure);
    window.addEventListener("scroll", measure, true);
    window.visualViewport?.addEventListener("resize", measure);
    measure();

    return () => {
      cancelAnimationFrame(animationFrame);
      observer.disconnect();
      resolutionQuery.removeEventListener("change", handleResolutionChange);
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
      window.visualViewport?.removeEventListener("resize", measure);
    };
  }, [onBoundsChange, serviceId]);

  return (
    <section
      id="service-webview-panel"
      className="service-webview-panel"
      role="tabpanel"
      aria-label={t("serviceView.panelLabel", { serviceName })}
    >
      <div ref={viewportRef} className="service-webview-viewport" />
      <div className="service-webview-placeholder" aria-live="polite">
        {status === "error" ? (
          <div className="service-webview-error" role="alert">
            <span>{t("serviceView.unavailable")}</span>
            <strong>{serviceName}</strong>
            <p>{error}</p>
          </div>
        ) : (
          <div className="service-webview-loading">
            <span className="service-webview-loading__spinner" aria-hidden="true" />
            <strong>{serviceName}</strong>
            <p>
              {status === "ready"
                ? t("serviceView.ready")
                : t("serviceView.opening")}
            </p>
          </div>
        )}
      </div>
    </section>
  );
}
