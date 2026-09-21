import assert from "node:assert/strict";
import { test } from "node:test";
import {
  parseServiceDiscoveryResponse,
  parseServerInventoryDiscoveryResponse,
} from "../node_modules/.cache/personal-hub-tests/features/discovery/discoveryClient.js";
import {
  createDiscoveryReviewItems,
  selectedDiscoveryServices,
} from "../node_modules/.cache/personal-hub-tests/features/discovery/discoveryModel.js";

function candidate(overrides = {}) {
  return {
    sourceId: "docker:jellyfin:8096",
    name: "Jellyfin",
    description: null,
    url: "http://10.0.0.10:8096/",
    iconHint: "jellyfin",
    ...overrides,
  };
}

function inventory() {
  return {
    address: "10.0.0.10",
    discovery: {
      source: "server-agent",
      sourceServiceId: "server-agent",
      candidates: [candidate()],
      skippedCount: 0,
    },
    sources: ["docker", "podman"].map((runtime) => ({
      runtime, scope: "rootful", state: "ready", ageSeconds: 10,
      skippedCount: 0, containerCount: 1,
    })),
    maintenance: { rebootRequired: null },
  };
}

test("agent inventory preserves exact DTO, inferred URLs and unknown maintenance", () => {
  for (const rebootRequired of [true, null]) {
    const value = inventory();
    value.maintenance.rebootRequired = rebootRequired;
    assert.deepEqual(parseServerInventoryDiscoveryResponse(value), value);
  }
  for (const url of [null, "http://10.0.0.10:80/", "https://10.0.0.10:443/", "https://10.0.0.10:65535/"]) {
    const value = inventory();
    value.discovery.candidates = [candidate({ url })];
    assert.equal(parseServerInventoryDiscoveryResponse(value).discovery.candidates[0].url, url);
  }
});

test("missing, stale and failed inventories are visible without invented services", () => {
  for (const state of ["missing", "stale", "unavailable", "invalid"]) {
    const value = inventory();
    value.discovery.candidates = [];
    value.sources = value.sources.map((source) => ({
      ...source, state, ageSeconds: state === "stale" ? 500 : null, containerCount: 0,
    }));
    assert.deepEqual(parseServerInventoryDiscoveryResponse(value), value);
  }
});

test("agent inventory rejects wrong targets, URL rewrites and unsafe protocols", () => {
  for (const url of [
    "http://127.0.0.1:8096/", "http://192.168.0.14:8096/",
    "http://10.0.0.10:0/", "http://10.0.0.10:65536/", "http://10.0.0.10:080/",
    "http://10.0.0.10/", "http://10.0.0.10:8096", "http://10.0.0.10:8096/?token=secret",
    "http://10.0.0.10:8096/#secret", "http://user:secret@10.0.0.10:8096/",
    "http://10.0.0.10:8096/web/", "http://10.0.0.10:8096/%2f",
    "http://10.0.0.10:8096/../", "http://3232235533:8096/",
    "https://10.0.0.10.evil.example:8096/", "file:///etc/passwd", "javascript:alert(1)",
  ]) {
    const value = inventory();
    value.discovery.candidates = [candidate({ url })];
    assert.throws(() => parseServerInventoryDiscoveryResponse(value), undefined, url);
  }
});

test("agent inventory rejects secrets or extra fields at every boundary", () => {
  for (const mutate of [
    (value) => { value.token = "secret"; },
    (value) => { value.discovery.environment = { PASSWORD: "secret" }; },
    (value) => { value.discovery.candidates[0].mounts = ["/etc"]; },
    (value) => { value.sources[0].environment = { PASSWORD: "secret" }; },
    (value) => { value.maintenance.packages = []; },
    (value) => { delete value.sources[0].scope; },
    (value) => { delete value.maintenance.rebootRequired; },
  ]) {
    const value = inventory();
    mutate(value);
    assert.throws(() => parseServerInventoryDiscoveryResponse(value));
  }
});

test("agent inventory rejects unexpected sources, statuses and unsafe bounds", () => {
  for (const mutate of [
    (value) => { value.discovery.source = "homarr"; },
    (value) => { value.discovery.sourceServiceId = "other-server"; },
    (value) => { value.sources.pop(); },
    (value) => { value.sources[1].runtime = "docker"; },
    (value) => { value.sources[1].runtime = "containerd"; },
    (value) => { value.sources[1].scope = "rootless"; },
    (value) => { value.sources[0].state = "probably-ready"; },
    (value) => { value.sources[0].ageSeconds = -1; },
    (value) => { value.sources[0].ageSeconds = Number.MAX_SAFE_INTEGER + 1; },
    (value) => { value.sources[0].containerCount = 65; },
    (value) => { value.sources[0].skippedCount = 100_001; },
    (value) => { value.discovery.skippedCount = 0.5; },
    (value) => { value.discovery.candidates = Array(257).fill(candidate()); },
    (value) => { value.discovery.candidates[0].iconHint = "https://example.org/logo.svg"; },
    (value) => { value.discovery.candidates[0].iconHint = "unknown-logo"; },
    (value) => { value.maintenance.rebootRequired = false; },
  ]) {
    const value = inventory();
    mutate(value);
    assert.throws(() => parseServerInventoryDiscoveryResponse(value));
  }
});

test("Homarr retains its separate strict contract and general web URLs", () => {
  const value = {
    source: "homarr", sourceServiceId: "my-homarr", skippedCount: 1,
    candidates: [candidate({ url: "https://media.example.org/web", iconHint: "jellyfin.svg" })],
  };
  assert.deepEqual(parseServiceDiscoveryResponse(value), value);
  assert.throws(() => parseServiceDiscoveryResponse(inventory().discovery));
  assert.throws(() => parseServiceDiscoveryResponse({ ...value, environment: {} }));
  assert.throws(() => parseServiceDiscoveryResponse({ ...value, candidates: Array(501).fill(candidate()) }));
});

test("manual-review candidates cannot be imported and additions recheck latest duplicates", () => {
  const candidates = [candidate(), candidate({ sourceId: "docker:plex", name: "Plex", url: null, iconHint: "plex" })];
  const initial = createDiscoveryReviewItems(candidates, [], "server-agent");
  const selection = new Set(initial.map((item) => item.key));
  const selected = selectedDiscoveryServices(candidates, [], selection, "server-agent");
  assert.equal(selected.length, 1);
  assert.equal(selected[0].description, "Server Agent");
  assert.equal(initial[1].service, null);
  assert.equal(selectedDiscoveryServices(candidates, [], new Set(), "server-agent").length, 0);
  assert.equal(selectedDiscoveryServices(candidates, selected, selection, "server-agent").length, 0);
  const equivalent = { ...selected[0], id: "existing", name: "Media", url: "http://10.0.0.10:8096/#tab" };
  assert.equal(selectedDiscoveryServices(candidates, [equivalent], selection, "server-agent").length, 0);
});

test("the inventory address binds every candidate and rejects non-private hosts", () => {
  // A candidate on any host other than the declared address is refused, so a
  // native response cannot smuggle in a service that is not on this server.
  for (const address of ["10.0.0.10", "10.0.0.5", "172.16.4.2", "127.0.0.1"]) {
    const value = inventory();
    value.address = address;
    value.discovery.candidates = [candidate({ url: `http://${address}:8096/` })];
    assert.equal(parseServerInventoryDiscoveryResponse(value).address, address);
    const mismatched = inventory();
    mismatched.address = address;
    mismatched.discovery.candidates = [candidate({ url: "http://192.168.99.99:8096/" })];
    assert.throws(() => parseServerInventoryDiscoveryResponse(mismatched), /service target was invalid/);
  }
  for (const address of ["", "8.8.8.8", "example.com", "10.0.0.10:9473", "192.168.1.999", null, 13]) {
    const value = inventory();
    value.address = address;
    assert.throws(() => parseServerInventoryDiscoveryResponse(value), /address was invalid/, String(address));
  }
});
