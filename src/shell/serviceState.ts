import type { ServiceHealth } from "../features/health/health.types";

/**
 * The single place that decides what counts as a problem.
 *
 * The previous dashboard treated every non-online state as one, so a server
 * whose four self-signed services were working exactly as configured reported
 * "4 need attention" on every launch. A warning the user deliberately opted
 * into is not news, and neither is a check that cannot run at all outside the
 * desktop runtime.
 */
export type ServiceState = "ok" | "attention" | "down" | "pending";

export function serviceState(health: ServiceHealth | undefined): ServiceState {
  if (!health || health.status === "unchecked") {
    return "pending";
  }

  if (health.status === "offline") {
    return "down";
  }

  if (health.status === "warning") {
    // An accepted private certificate is the configured outcome, not a fault:
    // the service answered, just over a certificate the user chose to trust.
    if (health.reason === "tls-exception") {
      return "ok";
    }

    // A check that could not run at all outside the desktop runtime is not a
    // problem either, but it is not confirmation. Reporting it as online would
    // claim something nobody verified.
    if (health.reason === "runtime") {
      return "pending";
    }

    return "attention";
  }

  return "ok";
}

export function needsAttention(health: ServiceHealth | undefined): boolean {
  const state = serviceState(health);
  return state === "attention" || state === "down";
}

export const STATE_DOT: Readonly<Record<ServiceState, string>> = {
  ok: "dot dot--ok",
  attention: "dot dot--warn",
  down: "dot dot--bad",
  pending: "dot",
};
