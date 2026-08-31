import assert from "node:assert/strict";
import { test } from "node:test";

import { CatalogRequestGeneration } from "../node_modules/.cache/personal-hub-tests/features/services/hooks/catalogRequestGeneration.js";
import { parseServiceConfiguration } from "../node_modules/.cache/personal-hub-tests/features/services/config/serviceConfig.js";
import {
  isTransientSettingsStartupError,
  parseServiceAuthenticationStatus,
  parseServiceConfigurationSnapshot,
  parseServiceCredentialStatuses,
  runWithSettingsStartupRetry,
} from "../node_modules/.cache/personal-hub-tests/features/settings/settingsClient.js";

function authenticationStatus(overrides = {}) {
  return {
    serviceId: "homarr",
    revision: "v1:7:12",
    apiAdapter: "homarr-api-key",
    browserAdapter: "none",
    requiredCredentialKinds: ["api-key"],
    credentialState: "stored",
    validationState: "valid",
    canValidate: true,
    canClearSession: false,
    reasonCode: null,
    retryAfterMs: null,
    ...overrides,
  };
}

function configuration(overrides = {}) {
  return {
    version: 1,
    services: [
      {
        id: "jellyfin",
        name: "Jellyfin",
        description: "Media server",
        url: "http://192.168.1.10:8096",
        icon: "jellyfin",
        category: "Media",
        enabled: true,
        accent: "violet",
        tlsPolicy: "strict",
        ...overrides,
      },
    ],
  };
}

test("settings snapshots accept only the bounded public response shape", () => {
  const parsed = parseServiceConfigurationSnapshot({
    configuration: configuration(),
    recoveryNotice: null,
    backupAvailable: true,
  });

  assert.equal(parsed.configuration.services[0].id, "jellyfin");
  assert.equal(parsed.backupAvailable, true);
  assert.throws(
    () =>
      parseServiceConfigurationSnapshot({
        configuration: configuration(),
        recoveryNotice: null,
        backupAvailable: true,
        secret: "must never be accepted",
      }),
    /unexpected response shape/,
  );
});

test("credential status parsing canonicalizes kinds and never accepts metadata", () => {
  const parsed = parseServiceCredentialStatuses([
    { kind: "username-password", exists: false },
    { kind: "api-key", exists: true },
    { kind: "http-basic", exists: false },
    { kind: "bearer-token", exists: true },
  ]);

  assert.deepEqual(parsed, [
    { kind: "api-key", exists: true },
    { kind: "bearer-token", exists: true },
    { kind: "http-basic", exists: false },
    { kind: "username-password", exists: false },
  ]);
  assert.throws(
    () =>
      parseServiceCredentialStatuses([
        { kind: "api-key", exists: true, username: "leak" },
        { kind: "bearer-token", exists: false },
        { kind: "http-basic", exists: false },
        { kind: "username-password", exists: false },
      ]),
    /unexpected response shape/,
  );
});

test("authentication snapshots accept the exact bounded enum shape", () => {
  const parsed = parseServiceAuthenticationStatus(authenticationStatus());

  assert.deepEqual(parsed, authenticationStatus());
  assert.throws(
    () =>
      parseServiceAuthenticationStatus(
        authenticationStatus({ token: "must never be accepted" }),
      ),
    /unexpected response shape/,
  );
  assert.throws(
    () =>
      parseServiceAuthenticationStatus(
        authenticationStatus({ validationState: "probably-valid" }),
      ),
    /invalid value/,
  );
  assert.throws(
    () =>
      parseServiceAuthenticationStatus(
        authenticationStatus({ revision: " v1:7:12" }),
      ),
    /invalid revision/,
  );
});

test("authentication snapshot invariants bind adapters to canonical credentials", () => {
  assert.deepEqual(
    parseServiceAuthenticationStatus(
      authenticationStatus({
        apiAdapter: "glances-http-basic",
        browserAdapter: "http-basic",
        requiredCredentialKinds: ["http-basic"],
      }),
    ),
    authenticationStatus({
      apiAdapter: "glances-http-basic",
      browserAdapter: "http-basic",
      requiredCredentialKinds: ["http-basic"],
    }),
  );

  assert.throws(
    () =>
      parseServiceAuthenticationStatus(
        authenticationStatus({ requiredCredentialKinds: ["bearer-token"] }),
      ),
    /required credential kinds/,
  );
  assert.throws(
    () =>
      parseServiceAuthenticationStatus(
        authenticationStatus({
          apiAdapter: "homarr-api-key",
          browserAdapter: "http-basic",
          requiredCredentialKinds: ["http-basic", "api-key"],
        }),
      ),
    /required credential kinds/,
  );
  assert.throws(
    () =>
      parseServiceAuthenticationStatus(
        authenticationStatus({
          apiAdapter: "glances-bearer",
          browserAdapter: "http-basic",
          requiredCredentialKinds: ["bearer-token", "http-basic"],
        }),
      ),
    /inconsistent/,
  );
});

test("authentication snapshot invariants reject impossible state combinations", () => {
  const unsupported = authenticationStatus({
    apiAdapter: "none",
    browserAdapter: "none",
    requiredCredentialKinds: [],
    credentialState: "not-required",
    validationState: "unsupported",
    canValidate: false,
  });
  assert.deepEqual(parseServiceAuthenticationStatus(unsupported), unsupported);

  const invalidCases = [
    authenticationStatus({
      apiAdapter: "none",
      browserAdapter: "none",
      requiredCredentialKinds: [],
      credentialState: "stored",
      validationState: "valid",
    }),
    authenticationStatus({
      credentialState: "missing",
      validationState: "not-validated",
      reasonCode: "missing-credential",
      canValidate: true,
    }),
    authenticationStatus({
      credentialState: "needs-rebind",
      validationState: "valid",
      reasonCode: "endpoint-changed",
      canValidate: false,
    }),
    authenticationStatus({
      validationState: "backoff",
      reasonCode: "rate-limited",
      retryAfterMs: 0,
      canValidate: false,
    }),
    authenticationStatus({ retryAfterMs: 1_000 }),
    authenticationStatus({
      validationState: "invalid",
      reasonCode: "timeout",
    }),
    authenticationStatus({
      validationState: "temporarily-unavailable",
      reasonCode: null,
    }),
  ];

  for (const value of invalidCases) {
    assert.throws(
      () => parseServiceAuthenticationStatus(value),
      /authentication response was inconsistent/,
    );
  }
});

test("authentication reason codes match their credential and validation states", () => {
  const validCases = [
    authenticationStatus({
      credentialState: "missing",
      validationState: "not-validated",
      reasonCode: "missing-credential",
      canValidate: false,
    }),
    authenticationStatus({
      credentialState: "needs-rebind",
      validationState: "not-validated",
      reasonCode: "endpoint-changed",
      canValidate: false,
    }),
    authenticationStatus({
      validationState: "invalid",
      reasonCode: "unauthorized",
    }),
    authenticationStatus({
      validationState: "temporarily-unavailable",
      reasonCode: "insecure-transport",
    }),
    authenticationStatus({
      validationState: "backoff",
      reasonCode: "rate-limited",
      retryAfterMs: 2_000,
      canValidate: false,
    }),
    authenticationStatus({
      validationState: "backoff",
      reasonCode: "unauthorized",
      retryAfterMs: 2_000,
      canValidate: false,
    }),
  ];

  for (const value of validCases) {
    assert.deepEqual(parseServiceAuthenticationStatus(value), value);
  }
});

test("service settings reject credentials in JSON fields and URLs", () => {
  assert.throws(
    () =>
      parseServiceConfiguration({
        ...configuration(),
        services: [
          {
            ...configuration().services[0],
            apiKey: "must-not-be-stored",
          },
        ],
      }),
    /not a supported configuration field/,
  );
  assert.throws(
    () =>
      parseServiceConfiguration(
        configuration({ url: "http://admin:secret@192.168.1.10:8096" }),
      ),
    /must not contain embedded credentials/,
  );
});

test("the native settings load retries only the transient Tauri startup race", async () => {
  let attempts = 0;
  const waits = [];
  const transientError =
    "state not managed for field `settings` on command `get_service_configuration`. You must call `.manage()` before using this command";

  const result = await runWithSettingsStartupRetry(
    async () => {
      attempts += 1;
      if (attempts < 3) {
        throw transientError;
      }
      return "ready";
    },
    async (delay) => {
      waits.push(delay);
    },
  );

  assert.equal(result, "ready");
  assert.equal(attempts, 3);
  assert.deepEqual(waits, [25, 50]);
  assert.equal(isTransientSettingsStartupError(transientError), true);
  assert.equal(isTransientSettingsStartupError(new Error(transientError)), true);

  let permanentAttempts = 0;
  await assert.rejects(
    runWithSettingsStartupRetry(async () => {
      permanentAttempts += 1;
      throw new Error("permission denied");
    }),
    /permission denied/,
  );
  assert.equal(permanentAttempts, 1);
});

test("a persisted catalog invalidates an older in-flight snapshot", async () => {
  const generation = new CatalogRequestGeneration();
  const staleRequest = generation.begin();
  const applied = [];
  let resolveStaleRequest;
  const staleResult = new Promise((resolve) => {
    resolveStaleRequest = resolve;
  }).then((value) => {
    if (generation.isCurrent(staleRequest)) {
      applied.push(value);
    }
  });

  generation.invalidate();
  applied.push("persisted catalog");
  resolveStaleRequest("old snapshot");
  await staleResult;

  assert.deepEqual(applied, ["persisted catalog"]);
});
