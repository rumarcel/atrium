import type { DashboardService } from "../services/service.types";

export interface ServiceDiscoveryCandidate {
  sourceId: string;
  name: string;
  description: string | null;
  url: string | null;
  iconHint: string | null;
}
export interface ServiceDiscoveryResponse {
  source: "homarr" | "server-agent";
  sourceServiceId: string;
  candidates: readonly ServiceDiscoveryCandidate[];
  skippedCount: number;
}

export interface ServerInventorySource {
  runtime: "docker" | "podman";
  scope: "rootful";
  state: "ready" | "missing" | "stale" | "unavailable" | "invalid";
  ageSeconds: number | null;
  skippedCount: number;
  containerCount: number;
}

export interface ServerInventoryDiscoveryResponse {
  /// The enrolled server address every candidate URL must point at.
  address: string;
  discovery: ServiceDiscoveryResponse;
  sources: readonly ServerInventorySource[];
  maintenance: { rebootRequired: true | null };
}

export interface ServiceDiscoveryClient {
  discoverHomarrServices: (serviceId: string) => Promise<ServiceDiscoveryResponse>;
  discoverServerInventory: () => Promise<ServerInventoryDiscoveryResponse>;
}

export interface ServiceDiscoveryReviewItem {
  key: string;
  candidate: ServiceDiscoveryCandidate;
  service: DashboardService | null;
  iconConfidence: "high" | "medium" | "low";
  duplicateOf: string | null;
}
