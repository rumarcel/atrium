import assert from "node:assert/strict";
import { test } from "node:test";

import { parseDownloadCenterResult } from "../node_modules/.cache/personal-hub-tests/features/downloads/downloadsClient.js";

function item(overrides = {}) {
  return {
    id: "torrent-1",
    name: "Ubuntu ISO",
    progressPercent: 42.5,
    downloadSpeedBytesPerSecond: 1024,
    etaSeconds: 60,
    state: "downloading",
    category: null,
    tags: [],
    source: null,
    ...overrides,
  };
}

function snapshot(overrides = {}) {
  return {
    status: "online",
    providerState: "configured",
    reason: null,
    sampledAt: 1_700_000_000_000,
    retryAfterMs: null,
    provider: { serviceId: "qbittorrent", name: "qBittorrent" },
    totalDownloadSpeedBytesPerSecond: 1024,
    items: [item()],
    message: null,
    ...overrides,
  };
}

test("uncategorized downloads cross the native boundary as null", () => {
  const result = parseDownloadCenterResult(snapshot());
  assert.equal(result.items[0].category, null);
  assert.deepEqual(result, snapshot());
  assert.throws(
    () => parseDownloadCenterResult(snapshot({ items: [item({ category: "" })] })),
    /category was invalid/,
  );
});

test("download responses reject unknown fields and credentials at every boundary", () => {
  const source = { kind: "sonarr", serviceId: "sonarr", label: "Sonarr" };
  for (const payload of [
    snapshot({ sessionCookie: "secret" }),
    snapshot({ provider: { ...snapshot().provider, password: "secret" } }),
    snapshot({ items: [item({ savePath: "private-path" })] }),
    snapshot({ items: [item({ source: { ...source, apiKey: "secret" } })] }),
  ]) {
    assert.throws(
      () => parseDownloadCenterResult(payload),
      /unexpected response shape/,
    );
  }
});

test("backoff requires a positive retry delay and cannot retain native items", () => {
  const backoff = snapshot({
    status: "unavailable",
    reason: "backoff",
    retryAfterMs: 5000,
    totalDownloadSpeedBytesPerSecond: 0,
    items: [],
  });
  assert.equal(parseDownloadCenterResult(backoff).retryAfterMs, 5000);
  for (const retryAfterMs of [null, 0, -1, Number.POSITIVE_INFINITY]) {
    assert.throws(() => parseDownloadCenterResult({ ...backoff, retryAfterMs }));
  }
  assert.throws(
    () => parseDownloadCenterResult({ ...backoff, items: [item()] }),
    /inconsistent/,
  );
  assert.throws(
    () => parseDownloadCenterResult({ ...backoff, reason: "connection" }),
    /inconsistent/,
  );
});

test("download item limits reject oversized, duplicate and malformed payloads", () => {
  const items = Array.from({ length: 200 }, (_, index) =>
    item({ id: `torrent-${index}` }),
  );
  assert.equal(parseDownloadCenterResult(snapshot({ items })).items.length, 200);
  for (const invalidItems of [
    [...items, item({ id: "overflow" })],
    [item(), item()],
    [item({ progressPercent: 100.1 })],
    [item({ downloadSpeedBytesPerSecond: Number.NaN })],
    [item({ state: "unexpected" })],
    [item({ name: "a".repeat(257) })],
    [item({ tags: Array.from({ length: 17 }, (_, index) => `tag-${index}`) })],
  ]) {
    assert.throws(() =>
      parseDownloadCenterResult(snapshot({ items: invalidItems })),
    );
  }
});

test("an absent provider cannot expose cached downloads or provider identity", () => {
  const absent = snapshot({
    status: "unavailable",
    providerState: "not-configured",
    provider: null,
    totalDownloadSpeedBytesPerSecond: 0,
    items: [],
  });
  assert.equal(parseDownloadCenterResult(absent).provider, null);
  assert.throws(
    () => parseDownloadCenterResult({ ...absent, items: [item()] }),
    /inconsistent/,
  );
  assert.throws(
    () => parseDownloadCenterResult({ ...absent, provider: snapshot().provider }),
    /inconsistent/,
  );
});
