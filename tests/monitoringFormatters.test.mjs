import assert from "node:assert/strict";
import { test } from "node:test";

import { formatUptime } from "../node_modules/.cache/personal-hub-tests/features/monitoring/monitoringFormatters.js";

const cases = [
  [null, "Unavailable"],
  [0, "< 1m"],
  [59, "< 1m"],
  [60, "1m"],
  [3_599, "59m"],
  [3_600, "1h 0m"],
  [86_400, "1d 0h"],
  [Number.MAX_VALUE, "Over 285m years"],
];

for (const [seconds, expected] of cases) {
  test(`uptime ${String(seconds)} formats safely`, () => {
    assert.equal(formatUptime(seconds), expected);
  });
}

test("non-finite uptime is unavailable", () => {
  assert.equal(formatUptime(Number.POSITIVE_INFINITY), "Unavailable");
});
