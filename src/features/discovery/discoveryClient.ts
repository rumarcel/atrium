import { invoke, isTauri } from "@tauri-apps/api/core";
// The enrolled-address rule lives with server control, which owns that value.
import { isPrivateIpv4 } from "../serverControl/serverControl.types.js";
import type {
  ServiceDiscoveryCandidate,
  ServiceDiscoveryClient,
  ServiceDiscoveryResponse,
  ServerInventoryDiscoveryResponse,
  ServerInventorySource,
} from "./discovery.types";

const DISCOVER_HOMARR_COMMAND = "discover_homarr_services";
const DISCOVER_SERVER_INVENTORY_COMMAND = "discover_server_inventory";
const SERVER_ICON_HINTS = new Set([
  "jellyfin", "plex", "emby", "radarr", "sonarr", "bazarr", "prowlarr",
  "lidarr", "readarr", "immich", "navidrome", "audiobookshelf", "qbittorrent",
  "sabnzbd", "transmission", "metube", "homarr", "home-assistant", "nextcloud",
  "portainer", "glances", "grafana", "pihole", "adguard-home", "crafty",
  "gerbera", "docker", "podman",
]);
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
    throw new Error("A discovery candidate was invalid.");
  }
  assertExactKeys(value, CANDIDATE_KEYS, "A discovery candidate");
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

function boundedInteger(value: unknown, maximum: number, context: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > maximum) {
    throw new Error(`${context} was invalid.`);
  }
  return value;
}

function parseServerCandidate(value: unknown, address: string): ServiceDiscoveryCandidate {
  const candidate = parseCandidate(value);
  if (candidate.iconHint !== null && !SERVER_ICON_HINTS.has(candidate.iconHint)) {
    throw new Error("The server inventory icon hint was invalid.");
  }
  if (candidate.url !== null) {
    // Only native-derived, published web ports on the paired server are eligible.
    // A URL parser alone would normalize alternate hosts, encoded paths and ports,
    // so the shape is matched exactly and the host compared to the enrolled
    // address: no path beyond "/", credentials, query or fragment is accepted.
    const match = /^https?:\/\/([0-9.]+):([1-9][0-9]{0,4})\/$/.exec(candidate.url);
    if (match === null || match[1] !== address || Number(match[2]) > 65_535) {
      throw new Error("The server inventory service target was invalid.");
    }
  }
  return candidate;
}

function parseInventorySource(value: unknown): ServerInventorySource {
  if (!isRecord(value)) {
    throw new Error("The inventory source was invalid.");
  }
  assertExactKeys(value, new Set([
    "runtime", "scope", "state", "ageSeconds", "skippedCount", "containerCount",
  ]), "The inventory source");
  if (value.runtime !== "docker" && value.runtime !== "podman") {
    throw new Error("The inventory runtime was invalid.");
  }
  if (value.scope !== "rootful") {
    throw new Error("The inventory scope was invalid.");
  }
  if (
    value.state !== "ready" && value.state !== "missing" && value.state !== "stale" &&
    value.state !== "unavailable" && value.state !== "invalid"
  ) {
    throw new Error("The inventory state was invalid.");
  }
  return {
    runtime: value.runtime,
    scope: value.scope,
    state: value.state,
    ageSeconds: value.ageSeconds === null ? null : boundedInteger(value.ageSeconds, Number.MAX_SAFE_INTEGER, "The inventory age"),
    skippedCount: boundedInteger(value.skippedCount, 100_000, "The inventory skipped count"),
    containerCount: boundedInteger(value.containerCount, 64, "The inventory container count"),
  };
}

export function parseServerInventoryDiscoveryResponse(value: unknown): ServerInventoryDiscoveryResponse {
  if (!isRecord(value)) {
    throw new Error("The server inventory response was invalid.");
  }
  assertExactKeys(value, new Set(["address", "discovery", "sources", "maintenance"]), "The server inventory response");
  // Every candidate URL is checked against this one declared host, so an
  // address that is not a bare private literal invalidates the whole response.
  if (typeof value.address !== "string" || !isPrivateIpv4(value.address)) {
    throw new Error("The server inventory address was invalid.");
  }
  const address = value.address;
  if (!isRecord(value.discovery)) {
    throw new Error("The server inventory discovery was invalid.");
  }
  assertExactKeys(value.discovery, RESPONSE_KEYS, "The server inventory discovery");
  if (value.discovery.source !== "server-agent" || value.discovery.sourceServiceId !== "server-agent") {
    throw new Error("The server inventory source was invalid.");
  }
  if (!Array.isArray(value.discovery.candidates) || value.discovery.candidates.length > 256) {
    throw new Error("The server inventory candidate list was invalid.");
  }
  if (!Array.isArray(value.sources) || value.sources.length !== 2) {
    throw new Error("The inventory sources were invalid.");
  }
  const sources = value.sources.map(parseInventorySource);
  if (new Set(sources.map((source) => source.runtime)).size !== 2) {
    throw new Error("The inventory sources were duplicated.");
  }
  if (!isRecord(value.maintenance)) {
    throw new Error("The inventory maintenance status was invalid.");
  }
  assertExactKeys(value.maintenance, new Set(["rebootRequired"]), "The inventory maintenance status");
  if (value.maintenance.rebootRequired !== true && value.maintenance.rebootRequired !== null) {
    throw new Error("The inventory maintenance status was invalid.");
  }
  return {
    address,
    discovery: {
      source: "server-agent",
      sourceServiceId: "server-agent",
      candidates: value.discovery.candidates.map((candidate) => parseServerCandidate(candidate, address)),
      skippedCount: boundedInteger(value.discovery.skippedCount, 100_000, "The discovery skipped count"),
    },
    sources,
    maintenance: { rebootRequired: value.maintenance.rebootRequired },
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
  discoverServerInventory,
};

export async function discoverServerInventory(): Promise<ServerInventoryDiscoveryResponse> {
  if (!isTauri()) {
    throw new Error("Service discovery is available only in the desktop app.");
  }
  return parseServerInventoryDiscoveryResponse(
    await invoke<unknown>(DISCOVER_SERVER_INVENTORY_COMMAND),
  );
}

export function describeDiscoveryError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.trim().slice(0, 500);
  }
  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }
  return "Service discovery could not be completed.";
}
