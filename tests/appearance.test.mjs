import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import {
  AppearanceSchemaError,
  createDefaultAppearancePreferences,
  createThemePackDocument,
  parseAppearancePreferences,
  parseAppearanceSnapshot,
  parseThemePackDocument,
  serializeThemePackDocument,
} from "../node_modules/.cache/personal-hub-tests/features/appearance/appearanceSchema.js";
import {
  resolveAppearance,
  resolveColorMode,
  themeTokensToCssVariables,
} from "../node_modules/.cache/personal-hub-tests/features/appearance/appearanceRuntime.js";
import {
  BUNDLED_THEMES,
  THEME_STUDIO_FIELDS,
  normalizeThemeColor,
} from "../node_modules/.cache/personal-hub-tests/features/appearance/themeTokens.js";

function preferences(overrides = {}) {
  return {
    ...createDefaultAppearancePreferences(),
    ...overrides,
  };
}

test("first run preserves the original dark default appearance", () => {
  assert.deepEqual(createDefaultAppearancePreferences(), {
    version: 1,
    colorMode: "dark",
    themeId: "default",
    language: "system",
    overrides: { dark: {}, light: {} },
  });

  const appearance = resolveAppearance(createDefaultAppearancePreferences(), "light");
  assert.equal(appearance.colorModePreference, "dark");
  assert.equal(appearance.resolvedColorMode, "dark");
  assert.equal(appearance.themeId, "default");
  assert.deepEqual(appearance.tokens, BUNDLED_THEMES.default.dark);
});

test("snapshots and theme packs accept only their exact versioned schemas", () => {
  const inputPreferences = preferences({
    themeId: "code",
    overrides: {
      dark: { accent: "#aabbcc", density: 0.9 },
      light: {},
    },
  });
  const snapshot = parseAppearanceSnapshot({
    preferences: inputPreferences,
    revision: "v1:4:abc",
    recoveryNotice: null,
  });

  assert.equal(snapshot.preferences.overrides.dark.accent, "#AABBCC");
  assert.equal(snapshot.preferences.overrides.dark.density, 0.9);

  const document = createThemePackDocument(snapshot.preferences);
  assert.deepEqual(parseThemePackDocument(document), document);
  assert.deepEqual(
    JSON.parse(serializeThemePackDocument(document)),
    document,
  );

  assert.throws(
    () => parseAppearanceSnapshot({ ...snapshot, secret: "must-not-pass" }),
    AppearanceSchemaError,
  );
  assert.throws(
    () => parseThemePackDocument({ ...document, version: 2 }),
    AppearanceSchemaError,
  );
  assert.throws(
    () => parseThemePackDocument({ ...document, metadata: {} }),
    AppearanceSchemaError,
  );
  assert.throws(
    () => parseThemePackDocument({ kind: document.kind, version: 1 }),
    AppearanceSchemaError,
  );
});

test("appearance input rejects unknown fields, nulls, and unsafe CSS", () => {
  const invalidInputs = [
    null,
    { ...preferences(), unknown: true },
    preferences({ overrides: null }),
    preferences({ overrides: { dark: null, light: {} } }),
    preferences({ overrides: { dark: { fontFamily: "serif" }, light: {} } }),
    preferences({ overrides: { dark: { accent: "red" }, light: {} } }),
    preferences({ overrides: { dark: { accent: "var(--secret)" }, light: {} } }),
    preferences({ overrides: { dark: { accent: "url(https://example.test/x)" }, light: {} } }),
    preferences({ overrides: { dark: { accent: null }, light: {} } }),
    preferences({ overrides: { dark: { accent: "#12345" }, light: {} } }),
  ];

  for (const input of invalidInputs) {
    assert.throws(() => parseAppearancePreferences(input), AppearanceSchemaError);
  }
});

test("numeric safety bounds and ordered radii are enforced", () => {
  const invalidOverrides = [
    { density: 0.7 },
    { density: 1.3 },
    { blur: 41 },
    { radiusSmall: -1 },
    { radiusSmall: 4.5 },
    { radiusSmall: 20, radiusMedium: 10, radiusLarge: 30 },
    { radiusSmall: 4, radiusMedium: 16, radiusLarge: 12 },
  ];

  for (const dark of invalidOverrides) {
    assert.throws(
      () =>
        parseAppearancePreferences(
          preferences({ overrides: { dark, light: {} } }),
        ),
      AppearanceSchemaError,
    );
  }
});

test("theme values are canonicalized without accepting general CSS", () => {
  assert.equal(normalizeThemeColor(" #abc "), "#AABBCC");
  assert.equal(normalizeThemeColor("#1234"), "#11223344");
  assert.equal(normalizeThemeColor("rgba(1, 2, 3, .5)"), null);
  assert.equal(normalizeThemeColor("linear-gradient(red, blue)"), null);

  const parsed = parseAppearancePreferences(
    preferences({
      overrides: {
        dark: { accent: "#abcdef", radiusSmall: 3 },
        light: { background: "#01020380" },
      },
    }),
  );
  assert.deepEqual(parsed.overrides, {
    dark: { accent: "#ABCDEF", radiusSmall: 3 },
    light: { background: "#01020380" },
  });
});

test("the published theme-pack schema accepts lowercase and uppercase long hex input", () => {
  const schema = JSON.parse(
    readFileSync(
      new URL("../public/config/theme-pack.schema.json", import.meta.url),
      "utf8",
    ),
  );
  const pattern = new RegExp(schema.$defs.hexColor.pattern);

  for (const color of ["#abcdef", "#ABCDEF", "#aAbBcC80", "#010203FF"]) {
    assert.equal(pattern.test(color), true);
  }
  for (const color of ["#abc", "#abcd", "#12345g", "red"]) {
    assert.equal(pattern.test(color), false);
  }
});

test("dark, light, and system modes resolve deterministically", () => {
  assert.equal(resolveColorMode("dark", "light"), "dark");
  assert.equal(resolveColorMode("light", "dark"), "light");
  assert.equal(resolveColorMode("system", "dark"), "dark");
  assert.equal(resolveColorMode("system", "light"), "light");

  const systemLight = resolveAppearance(
    preferences({ colorMode: "system", themeId: "minimal" }),
    "light",
  );
  assert.equal(systemLight.resolvedColorMode, "light");
  assert.deepEqual(systemLight.tokens, BUNDLED_THEMES.minimal.light);
});

test("resolved tokens project to the bounded CSS variable contract", () => {
  const appearance = resolveAppearance(
    preferences({
      themeId: "code",
      overrides: { dark: { accent: "#ABCDEF", blur: 3 }, light: {} },
    }),
    "dark",
  );
  const variables = themeTokensToCssVariables(appearance.tokens);

  assert.equal(variables["--color-accent"], "#ABCDEF");
  assert.equal(variables["--theme-blur"], "3px");
  assert.equal(variables["--radius-small"], "4px");
  assert.equal(variables["--theme-density"], "0.9");
  assert.match(variables["--font-family-app"], /Cascadia/);
  assert.equal(Object.keys(variables).length, 23);
  assert.equal(Object.hasOwn(variables, "--custom-css"), false);
});

test("theme studio exposes pixel units for radii and blur only", () => {
  const numberFields = Object.fromEntries(
    THEME_STUDIO_FIELDS.filter((field) => field.kind === "number").map(
      (field) => [field.token, field.unit],
    ),
  );

  assert.deepEqual(numberFields, {
    radiusSmall: "px",
    radiusMedium: "px",
    radiusLarge: "px",
    density: "ratio",
    typeScale: "ratio",
    shadow: "ratio",
    translucency: "ratio",
    blur: "px",
  });
});

// Every bundled palette shipped tertiary text below the WCAG AA contrast
// minimum, on every surface it is drawn on. This locks the fix in: a future
// palette edit that dips back under 4.5:1 fails here instead of shipping.
function channels(colour) {
  const rgba = colour.match(/rgba?\(([^)]+)\)/);
  if (rgba) {
    const parts = rgba[1].split(",").map((part) => Number(part.trim()));
    return { rgb: parts.slice(0, 3), alpha: parts.length > 3 ? parts[3] : 1 };
  }
  const value = Number.parseInt(colour.replace("#", ""), 16);
  return { rgb: [(value >> 16) & 255, (value >> 8) & 255, value & 255], alpha: 1 };
}

function flatten(colour, backdrop) {
  const { rgb, alpha } = channels(colour);
  const base = channels(backdrop).rgb;
  return rgb.map((part, index) => part * alpha + base[index] * (1 - alpha));
}

function luminance(rgb) {
  const linear = rgb.map((part) => {
    const ratio = part / 255;
    return ratio <= 0.03928 ? ratio / 12.92 : Math.pow((ratio + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

function contrast(foreground, background, backdrop) {
  const a = luminance(flatten(foreground, backdrop));
  const b = luminance(flatten(background, backdrop));
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

test("every bundled palette keeps body text above the WCAG AA contrast minimum", () => {
  for (const [themeId, theme] of Object.entries(BUNDLED_THEMES)) {
    for (const mode of ["dark", "light"]) {
      const tokens = theme[mode];
      const surfaces = [tokens.background, tokens.surface, tokens.surfaceElevated];
      for (const role of ["textPrimary", "textSecondary", "textTertiary"]) {
        for (const surface of surfaces) {
          const ratio = contrast(tokens[role], surface, tokens.background);
          assert.ok(
            ratio >= 4.5,
            `${themeId}/${mode} ${role} on ${surface} is ${ratio.toFixed(2)}:1`,
          );
        }
      }
    }
  }
});
