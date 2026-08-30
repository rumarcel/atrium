import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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

interface InFlightCheck {
  generation: number;
  promise: Promise<void>;
}

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
  const [isChecking, setIsChecking] = useState(false);
  const [lastCheckedAt, setLastCheckedAt] = useState<number | null>(null);
  const [manualRevision, setManualRevision] = useState(0);
  const generationRef = useRef(0);
  const inFlightRef = useRef<InFlightCheck | null>(null);
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

  const performChecks = useCallback(
    (serviceSnapshot: readonly DashboardService[], generation: number) => {
      const currentCheck = inFlightRef.current;

      if (currentCheck?.generation === generation) {
        return currentCheck.promise;
      }

      setIsChecking(true);
      setHealthById((currentHealth) => {
        const nextHealth: Record<string, ServiceHealth> = {};

        for (const service of serviceSnapshot) {
          const existingHealth = currentHealth[service.id];
          nextHealth[service.id] = existingHealth
            ? { ...existingHealth, isChecking: true }
            : makeUncheckedHealth(service);
        }

        return nextHealth;
      });

      let task: Promise<void>;
      task = checkServicesWithLimit(serviceSnapshot)
        .then((results) => {
          if (generationRef.current !== generation) {
            return;
          }

          const nextHealth: Record<string, ServiceHealth> = {};
          let newestCheck = 0;

          for (const result of results) {
            nextHealth[result.serviceId] = { ...result, isChecking: false };
            newestCheck = Math.max(newestCheck, result.checkedAtUnixMs);
          }

          setHealthById(nextHealth);
          setLastCheckedAt(newestCheck || Date.now());
        })
        .finally(() => {
          if (generationRef.current === generation) {
            setIsChecking(false);
          }

          if (inFlightRef.current?.promise === task) {
            inFlightRef.current = null;
          }
        });

      inFlightRef.current = { generation, promise: task };
      return task;
    },
    [],
  );

  useEffect(() => {
    const configurationChanged =
      previousFingerprintRef.current !== configurationFingerprint;
    previousFingerprintRef.current = configurationFingerprint;
    generationRef.current += 1;
    const generation = generationRef.current;

    if (configurationChanged) {
      setHealthById({});
      setLastCheckedAt(null);
    }

    if (!enabled || services.length === 0) {
      setIsChecking(false);
      return;
    }

    let disposed = false;
    let pollTimer: number | undefined;

    const poll = async () => {
      await performChecks(services, generation);

      if (!disposed && generationRef.current === generation) {
        pollTimer = window.setTimeout(poll, HEALTH_POLL_INTERVAL_MS);
      }
    };

    const startupTimer = window.setTimeout(() => void poll(), 0);

    return () => {
      disposed = true;
      window.clearTimeout(startupTimer);

      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
      }
    };
  }, [configurationFingerprint, enabled, manualRevision, performChecks, services]);

  const refresh = useCallback(() => {
    if (inFlightRef.current?.generation === generationRef.current) {
      return;
    }

    setManualRevision((current) => current + 1);
  }, []);

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
    isChecking,
    lastCheckedAt,
    refresh,
    summary,
  };
}
