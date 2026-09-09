import { suggestServiceIcon } from "../services/icons/iconCatalog";
import type { DashboardService } from "../services/service.types";
import type {
  ServiceDiscoveryCandidate,
  ServiceDiscoveryReviewItem,
} from "./discovery.types";

function slug(value: string): string {
  const normalized = value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase("en-US")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 56);
  return normalized || "discovered-service";
}

function uniqueId(name: string, usedIds: Set<string>): string {
  const base = slug(name);
  let candidate = base;
  let suffix = 2;
  while (usedIds.has(candidate)) {
    const suffixText = `-${suffix}`;
    candidate = `${base.slice(0, 64 - suffixText.length)}${suffixText}`;
    suffix += 1;
  }
  usedIds.add(candidate);
  return candidate;
}

function canonicalUrl(value: string): string | null {
  try {
    const url = new URL(value);
    url.hash = "";
    if (url.pathname !== "/") {
      url.pathname = url.pathname.replace(/\/+$/, "");
    }
    return url.toString();
  } catch {
    return null;
  }
}

function duplicateService(
  candidate: ServiceDiscoveryCandidate,
  services: readonly DashboardService[],
): DashboardService | null {
  const candidateUrl = candidate.url ? canonicalUrl(candidate.url) : null;
  if (candidateUrl !== null) {
    const exact = services.find(
      (service) => canonicalUrl(service.url) === candidateUrl,
    );
    if (exact !== undefined) {
      return exact;
    }
  }
  const normalizedName = candidate.name.trim().toLocaleLowerCase();
  return (
    services.find(
      (service) => service.name.trim().toLocaleLowerCase() === normalizedName,
    ) ?? null
  );
}

export function createDiscoveryReviewItems(
  candidates: readonly ServiceDiscoveryCandidate[],
  existingServices: readonly DashboardService[],
): readonly ServiceDiscoveryReviewItem[] {
  const usedIds = new Set(existingServices.map((service) => service.id));
  const knownServices = [...existingServices];

  return candidates.map((candidate, index) => {
    const suggestion = suggestServiceIcon({
      name: candidate.name,
      url: candidate.url,
      iconHint: candidate.iconHint,
    });
    const duplicate = duplicateService(candidate, knownServices);
    const service: DashboardService | null = candidate.url
      ? {
          id: uniqueId(candidate.name, usedIds),
          name: candidate.name,
          description: candidate.description ?? "Homarr",
          url: candidate.url,
          category: suggestion.category,
          icon: suggestion.icon,
          accent: suggestion.accent,
          enabled: true,
          tlsPolicy: "strict",
          authentication: {
            api: "none",
            browser: "none",
            allowInsecureLocalHttp: false,
          },
        }
      : null;

    if (service !== null && duplicate === null) {
      knownServices.push(service);
    }
    return {
      key: `${candidate.sourceId}:${index}`,
      candidate,
      service,
      iconConfidence: suggestion.confidence,
      duplicateOf: duplicate?.name ?? null,
    };
  });
}
