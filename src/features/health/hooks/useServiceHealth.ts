import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVisibilityPolling } from "../../../hooks/useVisibilityPolling";
import type { DashboardService } from "../../services/service.types";
import { checkServiceHealth } from "../healthClient";
import type {
  HealthCheckResult,
  ServiceHealth,
  ServiceHealthById,
  ServiceHealthSummary,
} from "../health.types";

export const HEALTH_POLL_INTERVAL_MS = 45_000;
const MAX_CONCURRENT_CHECKS = 8;

const emptySummary: ServiceHealthSummary = {
  online: 0,
  offline: 0,
  warning: 0,
  unchecked: 0,
  total: 0,
};

function makeUncheckedHealth(service: DashboardService): ServiceHealth {
  return {
    serviceId: service.id,
    status: "unchecked",
    statusCode: null,
    latencyMs: 0,
    checkedAtUnixMs: 0,
    reason: null,
    message: null,
    tlsExceptionUsed: false,
    isChecking: true,
  };
}

function makeInvokeFailure(
  service: DashboardService,
  error: unknown,
): HealthCheckResult {
  const message =
    error instanceof Error && error.message.trim().length > 0
      ? error.message
      : "The native health check could not be completed.";

  return {
    serviceId: service.id,
    status: "offline",
    statusCode: null,
    latencyMs: 0,
    checkedAtUnixMs: Date.now(),
    reason: "invoke",
    message,
    tlsExceptionUsed: false,
  };
}

async function checkServicesWithLimit(
  services: readonly DashboardService[],
): Promise<readonly HealthCheckResult[]> {
  const results = new Array<HealthCheckResult>(services.length);
  let nextIndex = 0;

  async function worker() {
    while (nextIndex < services.length) {
      const index = nextIndex;
      nextIndex += 1;
      const service = services[index];

      try {
        results[index] = await checkServiceHealth(service);
      } catch (error) {
        results[index] = makeInvokeFailure(service, error);
      }
    }
  }

  const workerCount = Math.min(MAX_CONCURRENT_CHECKS, services.length);
  await Promise.all(Array.from({ length: workerCount }, () => worker()));
  return results;
}

export function useServiceHealth(
  services: readonly DashboardService[],
  enabled: boolean,
) {
  const [healthById, setHealthById] = useState<ServiceHealthById>({});
  const [lastCheckedAt, setLastCheckedAt] = useState<number | null>(null);
  const previousFingerprintRef = useRef<string | null>(null);

  const configurationFingerprint = useMemo(
    () =>
      services
        .map(
          (service) =>
            `${service.id}\u0000${service.url}\u0000${service.tlsPolicy}`,
        )
        .join("\u0001"),
    [services],
  );

  const performChecks = useCallback(() => {
      setHealthById((currentHealth) => {
        const nextHealth: Record<string, ServiceHealth> = {};

        for (const service of services) {
          const existingHealth = currentHealth[service.id];
          nextHealth[service.id] = existingHealth
            ? { ...existingHealth, isChecking: true }
            : makeUncheckedHealth(service);
        }

        return nextHealth;
      });

      return checkServicesWithLimit(services);
    }, [services]);

  const applyResults = useCallback((results: readonly HealthCheckResult[]) => {
    const nextHealth: Record<string, ServiceHealth> = {};
    let newestCheck = 0;

    for (const result of results) {
      nextHealth[result.serviceId] = { ...result, isChecking: false };
      newestCheck = Math.max(newestCheck, result.checkedAtUnixMs);
    }

    setHealthById(nextHealth);
    setLastCheckedAt(newestCheck || Date.now());
  }, []);

  const handlePollingError = useCallback(() => {
    setHealthById((currentHealth) =>
      Object.fromEntries(
        Object.entries(currentHealth).map(([serviceId, health]) => [
          serviceId,
          health ? { ...health, isChecking: false } : health,
        ]),
      ),
    );
  }, []);

  const polling = useVisibilityPolling({
    enabled: enabled && services.length > 0,
    intervalMs: HEALTH_POLL_INTERVAL_MS,
    revision: configurationFingerprint,
    poll: performChecks,
    onSuccess: applyResults,
    onError: handlePollingError,
  });

  useEffect(() => {
    const configurationChanged =
      previousFingerprintRef.current !== configurationFingerprint;
    previousFingerprintRef.current = configurationFingerprint;
    if (configurationChanged) {
      setHealthById({});
      setLastCheckedAt(null);
    }
  }, [configurationFingerprint]);

  const summary = useMemo(() => {
    if (services.length === 0) {
      return emptySummary;
    }

    const nextSummary: ServiceHealthSummary = {
      online: 0,
      offline: 0,
      warning: 0,
      unchecked: 0,
      total: services.length,
    };

    for (const service of services) {
      const status = healthById[service.id]?.status ?? "unchecked";
      nextSummary[status] += 1;
    }

    return nextSummary;
  }, [healthById, services]);

  return {
    healthById,
    isChecking: polling.isPolling,
    isPaused: polling.isPaused,
    lastCheckedAt,
    refresh: polling.refresh,
    summary,
  };
}
