import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  ServiceDiscoveryCandidate,
  ServiceDiscoveryClient,
  ServiceDiscoveryResponse,
} from "./discovery.types";

const DISCOVER_HOMARR_COMMAND = "discover_homarr_services";
const RESPONSE_KEYS = new Set([
  "source",
  "sourceServiceId",
  "candidates",
  "skippedCount",
]);
const CANDIDATE_KEYS = new Set([
  "sourceId",
  "name",
  "description",
  "url",
  "iconHint",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function assertExactKeys(
  value: Record<string, unknown>,
  keys: ReadonlySet<string>,
  context: string,
): void {
  const unexpected = Object.keys(value).find((key) => !keys.has(key));
  const missing = [...keys].find(
    (key) => !Object.prototype.hasOwnProperty.call(value, key),
  );
  if (unexpected !== undefined || missing !== undefined) {
    throw new Error(`${context} had an unexpected response shape.`);
  }
}

function boundedString(value: unknown, maximum: number, context: string): string {
  if (typeof value !== "string") {
    throw new Error(`${context} was invalid.`);
  }
  const normalized = value.trim();
  if (
    normalized.length < 1 ||
    normalized.length > maximum ||
    normalized !== value ||
    /[\u0000-\u001f\u007f]/.test(normalized)
  ) {
    throw new Error(`${context} was invalid.`);
  }
  return normalized;
}

function optionalString(
  value: unknown,
  maximum: number,
  context: string,
): string | null {
  return value === null ? null : boundedString(value, maximum, context);
}

function optionalUrl(value: unknown): string | null {
  if (value === null) {
    return null;
  }
  const text = boundedString(value, 2_048, "A discovered service URL");
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    throw new Error("A discovered service URL was invalid.");
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username.length > 0 ||
    url.password.length > 0
  ) {
    throw new Error("A discovered service URL was invalid.");
  }
  return text;
}

function parseCandidate(value: unknown): ServiceDiscoveryCandidate {
  if (!isRecord(value)) {
    throw new Error("A Homarr discovery candidate was invalid.");
  }
  assertExactKeys(value, CANDIDATE_KEYS, "A Homarr discovery candidate");
  return {
    sourceId: boundedString(value.sourceId, 200, "A discovery source ID"),
    name: boundedString(value.name, 80, "A discovered service name"),
    description: optionalString(
      value.description,
      160,
      "A discovered service description",
    ),
    url: optionalUrl(value.url),
    iconHint: optionalString(value.iconHint, 512, "A discovered icon hint"),
  };
}

export function parseServiceDiscoveryResponse(value: unknown): ServiceDiscoveryResponse {
  if (!isRecord(value)) {
    throw new Error("The service discovery response was invalid.");
  }
  assertExactKeys(value, RESPONSE_KEYS, "The service discovery response");
  if (value.source !== "homarr") {
    throw new Error("The service discovery source was invalid.");
  }
  if (!Array.isArray(value.candidates) || value.candidates.length > 500) {
    throw new Error("The service discovery candidate list was invalid.");
  }
  if (
    typeof value.skippedCount !== "number" ||
    !Number.isSafeInteger(value.skippedCount) ||
    value.skippedCount < 0 ||
    value.skippedCount > 100_000
  ) {
    throw new Error("The service discovery skipped count was invalid.");
  }
  return {
    source: "homarr",
    sourceServiceId: boundedString(
      value.sourceServiceId,
      64,
      "The discovery source service ID",
    ),
    candidates: value.candidates.map(parseCandidate),
    skippedCount: value.skippedCount,
  };
}

export async function discoverHomarrServices(
  serviceId: string,
): Promise<ServiceDiscoveryResponse> {
  if (!isTauri()) {
    throw new Error("Service discovery is available only in the desktop app.");
  }
  const response = await invoke<unknown>(DISCOVER_HOMARR_COMMAND, {
    request: { serviceId },
  });
  return parseServiceDiscoveryResponse(response);
}

export const nativeServiceDiscoveryClient: ServiceDiscoveryClient = {
  discoverHomarrServices,
};

export function describeDiscoveryError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.trim().slice(0, 500);
  }
  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }
  return "Service discovery could not be completed.";
}
