import { invoke, isTauri } from "@tauri-apps/api/core";
import type { DashboardService } from "../services/service.types";
import type { HealthCheckResult } from "./health.types";

const HEALTH_CHECK_COMMAND = "check_service_health";

export async function checkServiceHealth(
  service: DashboardService,
): Promise<HealthCheckResult> {
  if (!isTauri()) {
    return {
      serviceId: service.id,
      status: "warning",
      statusCode: null,
      latencyMs: 0,
      checkedAtUnixMs: Date.now(),
      reason: "runtime",
      message: "Health checks require the Tauri desktop runtime.",
      tlsExceptionUsed: false,
    };
  }

  const result = await invoke<HealthCheckResult>(HEALTH_CHECK_COMMAND, {
    request: {
      serviceId: service.id,
      url: service.url,
      tlsPolicy: service.tlsPolicy,
    },
  });

  if (result.serviceId !== service.id) {
    throw new Error("The native health-check response did not match the service.");
  }

  return result;
}
