import { useTranslation } from "../features/i18n";
import type { ServerMetricsMonitor } from "../features/monitoring/monitoring.types";

export interface VerdictProps {
  monitor: ServerMetricsMonitor;
  servicesDown: number;
  servicesAttention: number;
}

export type VerdictTone = "ok" | "warn" | "bad" | "busy" | "idle";

/** Thresholds the native notification runtime already alerts on, so the
 *  dashboard and the toast can never disagree about what counts as a problem. */
const WARNING_AT = { memory: 90, disk: 90, cpu: 95, temperature: 85 } as const;

/**
 * One sentence for the whole server.
 *
 * The old dashboard put four equally weighted numbers at the top of the screen
 * and left the reader to work out whether any of them mattered. The numbers
 * are still available below; this says whether they need looking at.
 */
export function useServerVerdict({
  monitor,
  servicesDown,
  servicesAttention,
}: VerdictProps): { tone: VerdictTone; text: string; detail: string | null } {
  const { percent, t, uptime } = useTranslation();
  const snapshot = monitor.snapshot;

  const uptimeText =
    snapshot?.uptimeSeconds == null
      ? null
      : t("verdict.uptime", { uptime: uptime(snapshot.uptimeSeconds) });

  // A service that is down is the most actionable thing on the screen, so it
  // outranks any reading the server itself reports.
  if (servicesDown > 0) {
    return {
      tone: "bad",
      text: t("verdict.servicesDown", { count: String(servicesDown) }),
      detail: uptimeText,
    };
  }

  if (monitor.status === "loading") {
    return { tone: "busy", text: t("verdict.connecting"), detail: null };
  }

  if (monitor.providerState === "not-configured") {
    return { tone: "idle", text: t("verdict.noMonitoring"), detail: null };
  }

  if (monitor.status !== "online") {
    return { tone: "bad", text: t("verdict.unreachable"), detail: null };
  }

  if (snapshot) {
    const disk = snapshot.disks.reduce((worst, item) => {
      const value =
        item.percent ??
        (item.totalBytes > 0 ? (item.usedBytes / item.totalBytes) * 100 : 0);
      return Math.max(worst, value);
    }, 0);
    const memoryTotal = snapshot.memoryTotalBytes ?? 0;
    const memoryUsed = snapshot.memoryUsedBytes ?? 0;
    const memory =
      snapshot.memoryPercent ?? (memoryTotal > 0 ? (memoryUsed / memoryTotal) * 100 : 0);

    if (disk >= WARNING_AT.disk) {
      return {
        tone: "warn",
        text: t("verdict.diskPressure", { percent: percent(disk) }),
        detail: uptimeText,
      };
    }

    if (memory >= WARNING_AT.memory) {
      return {
        tone: "warn",
        text: t("verdict.memoryPressure", { percent: percent(memory) }),
        detail: uptimeText,
      };
    }

    if (
      (snapshot.cpuPercent ?? 0) >= WARNING_AT.cpu ||
      (snapshot.cpuTemperatureC ?? 0) >= WARNING_AT.temperature
    ) {
      return { tone: "warn", text: t("verdict.cpuPressure"), detail: uptimeText };
    }
  }

  if (servicesAttention > 0) {
    return {
      tone: "warn",
      text: t("verdict.servicesAttention", { count: String(servicesAttention) }),
      detail: uptimeText,
    };
  }

  if (monitor.isStale) {
    return { tone: "warn", text: t("verdict.stale"), detail: uptimeText };
  }

  return { tone: "ok", text: t("verdict.allWell"), detail: uptimeText };
}
