const STORE_KEY = "personal-hub.custom-service-icons.v1";
const STORE_VERSION = 1;
const MAX_FILE_BYTES = 256 * 1024;
const MAX_DATA_URL_LENGTH = 360 * 1024;
const MAX_STORE_LENGTH = 2 * 1024 * 1024;
const MAX_ENTRIES = 32;
const MAX_DIMENSION = 2_048;

interface StoredIcon {
  dataUrl: string;
  updatedAt: number;
}

interface CustomIconStore {
  version: typeof STORE_VERSION;
  entries: Record<string, StoredIcon>;
}

const ALLOWED_SVG_ELEMENTS = new Set([
  "svg",
  "g",
  "path",
  "circle",
  "ellipse",
  "rect",
  "line",
  "polyline",
  "polygon",
  "title",
  "desc",
  "defs",
  "linearGradient",
  "radialGradient",
  "stop",
  "clipPath",
  "mask",
]);

function emptyStore(): CustomIconStore {
  return { version: STORE_VERSION, entries: {} };
}

function readStore(): CustomIconStore {
  if (typeof window === "undefined") {
    return emptyStore();
  }

  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(STORE_KEY) ?? "null");
    if (typeof value !== "object" || value === null) {
      return emptyStore();
    }
    const candidate = value as Partial<CustomIconStore>;
    if (
      candidate.version !== STORE_VERSION ||
      typeof candidate.entries !== "object" ||
      candidate.entries === null
    ) {
      return emptyStore();
    }

    const entries: Record<string, StoredIcon> = {};
    for (const [reference, entry] of Object.entries(candidate.entries)) {
      if (
        !/^custom:[a-f0-9]{32}$/.test(reference) ||
        typeof entry !== "object" ||
        entry === null
      ) {
        continue;
      }
      const stored = entry as Partial<StoredIcon>;
      if (
        typeof stored.dataUrl !== "string" ||
        stored.dataUrl.length > MAX_DATA_URL_LENGTH ||
        !/^data:image\/(?:webp|svg\+xml);base64,/.test(stored.dataUrl) ||
        typeof stored.updatedAt !== "number" ||
        !Number.isSafeInteger(stored.updatedAt)
      ) {
        continue;
      }
      entries[reference] = {
        dataUrl: stored.dataUrl,
        updatedAt: stored.updatedAt,
      };
    }
    return { version: STORE_VERSION, entries };
  } catch {
    return emptyStore();
  }
}

function randomReference(): string {
  const bytes = new Uint8Array(16);
  window.crypto.getRandomValues(bytes);
  return `custom:${[...bytes]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("")}`;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8_192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8_192));
  }
  return window.btoa(binary);
}

function parseSvgDimension(value: string | null): number | null {
  if (value === null || !/^\d+(?:\.\d+)?(?:px)?$/i.test(value.trim())) {
    return null;
  }
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function validateSvg(source: string): string {
  if (/<!doctype|<!entity/i.test(source)) {
    throw new Error("SVG document types and entities are not supported.");
  }
  const document = new DOMParser().parseFromString(source, "image/svg+xml");
  if (document.querySelector("parsererror") || document.documentElement.localName !== "svg") {
    throw new Error("The selected SVG is invalid.");
  }

  const elements = [document.documentElement, ...document.documentElement.querySelectorAll("*")];
  if (elements.length > 512) {
    throw new Error("The selected SVG is too complex.");
  }
  for (const element of elements) {
    if (
      element.namespaceURI !== "http://www.w3.org/2000/svg" ||
      !ALLOWED_SVG_ELEMENTS.has(element.localName)
    ) {
      throw new Error("The selected SVG contains unsupported content.");
    }
    if (element.attributes.length > 32) {
      throw new Error("The selected SVG is too complex.");
    }
    for (const attribute of element.getAttributeNames()) {
      const normalized = attribute.toLowerCase();
      if (
        normalized.startsWith("on") ||
        normalized === "style" ||
        normalized === "href" ||
        normalized.endsWith(":href")
      ) {
        throw new Error("The selected SVG contains active content.");
      }
      if (normalized === "xmlns" || normalized.startsWith("xmlns:")) {
        continue;
      }
      const attributeValue = element.getAttribute(attribute)?.toLowerCase() ?? "";
      const internalReference = /^url\(#[a-z_][a-z0-9_.:-]*\)$/i.test(
        attributeValue.trim(),
      );
      if (
        attributeValue.includes("javascript:") ||
        attributeValue.includes("data:") ||
        attributeValue.includes("http:") ||
        attributeValue.includes("https:") ||
        (attributeValue.includes("url(") && !internalReference)
      ) {
        throw new Error("The selected SVG contains an external reference.");
      }
    }
  }

  const root = document.documentElement;
  const viewBox = root.getAttribute("viewBox")
    ?.trim()
    .split(/[ ,]+/)
    .map(Number);
  const width = parseSvgDimension(root.getAttribute("width"));
  const height = parseSvgDimension(root.getAttribute("height"));
  const viewBoxIsValid =
    viewBox?.length === 4 &&
    viewBox.every(Number.isFinite) &&
    (viewBox[2] ?? 0) > 0 &&
    (viewBox[3] ?? 0) > 0 &&
    (viewBox[2] ?? 0) <= MAX_DIMENSION &&
    (viewBox[3] ?? 0) <= MAX_DIMENSION;
  if (
    !viewBoxIsValid &&
    (width === null || height === null || width > MAX_DIMENSION || height > MAX_DIMENSION)
  ) {
    throw new Error("The selected SVG needs bounded width and height or a valid viewBox.");
  }

  return new XMLSerializer().serializeToString(root);
}

async function validateWebp(bytes: Uint8Array): Promise<void> {
  const signature = String.fromCharCode(...bytes.subarray(0, 12));
  if (bytes.length < 16 || !signature.startsWith("RIFF") || signature.slice(8) !== "WEBP") {
    throw new Error("The selected file is not a valid WebP image.");
  }
  const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/webp" }));
  try {
    if (
      bitmap.width < 1 ||
      bitmap.height < 1 ||
      bitmap.width > MAX_DIMENSION ||
      bitmap.height > MAX_DIMENSION
    ) {
      throw new Error("The selected WebP dimensions are too large.");
    }
  } finally {
    bitmap.close();
  }
}

export async function importCustomServiceIcon(file: File): Promise<string> {
  if (file.size < 1 || file.size > MAX_FILE_BYTES) {
    throw new Error("Custom icons must be smaller than 256 KiB.");
  }

  let mimeType: "image/svg+xml" | "image/webp";
  let bytes = new Uint8Array(await file.arrayBuffer());
  if (file.type === "image/svg+xml" || file.name.toLowerCase().endsWith(".svg")) {
    mimeType = "image/svg+xml";
    let source: string;
    try {
      source = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      throw new Error("The selected SVG must use UTF-8 encoding.");
    }
    bytes = new TextEncoder().encode(validateSvg(source));
  } else if (file.type === "image/webp" || file.name.toLowerCase().endsWith(".webp")) {
    mimeType = "image/webp";
    await validateWebp(bytes);
  } else {
    throw new Error("Only SVG and WebP icons are supported.");
  }

  const reference = randomReference();
  const store = readStore();
  store.entries[reference] = {
    dataUrl: `data:${mimeType};base64,${bytesToBase64(bytes)}`,
    updatedAt: Date.now(),
  };
  if (store.entries[reference].dataUrl.length > MAX_DATA_URL_LENGTH) {
    throw new Error("The custom icon is too large for local storage.");
  }

  const oldestFirst = () =>
    Object.entries(store.entries).sort(([, left], [, right]) => left.updatedAt - right.updatedAt);
  while (Object.keys(store.entries).length > MAX_ENTRIES) {
    const oldest = oldestFirst()[0]?.[0];
    if (oldest === undefined || oldest === reference) {
      break;
    }
    delete store.entries[oldest];
  }
  while (JSON.stringify(store).length > MAX_STORE_LENGTH) {
    const oldest = oldestFirst().find(([key]) => key !== reference)?.[0];
    if (oldest === undefined) {
      throw new Error("The custom icon is too large for local storage.");
    }
    delete store.entries[oldest];
  }

  window.localStorage.setItem(STORE_KEY, JSON.stringify(store));
  return reference;
}

export function loadCustomServiceIcon(reference: string): string | null {
  if (!reference.startsWith("custom:")) {
    return null;
  }
  return readStore().entries[reference]?.dataUrl ?? null;
}
