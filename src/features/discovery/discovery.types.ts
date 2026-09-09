import type { DashboardService } from "../services/service.types";

export interface ServiceDiscoveryCandidate {
  sourceId: string;
  name: string;
  description: string | null;
  url: string | null;
  iconHint: string | null;
}
export interface ServiceDiscoveryResponse {
  source: "homarr";
  sourceServiceId: string;
  candidates: readonly ServiceDiscoveryCandidate[];
  skippedCount: number;
}

export interface ServiceDiscoveryClient {
  discoverHomarrServices: (serviceId: string) => Promise<ServiceDiscoveryResponse>;
}

export interface ServiceDiscoveryReviewItem {
  key: string;
  candidate: ServiceDiscoveryCandidate;
  service: DashboardService | null;
  iconConfidence: "high" | "medium" | "low";
  duplicateOf: string | null;
}
