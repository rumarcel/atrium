//! The route table: every route, and its policy, in one literal list.
//!
//! A route states, per mode, whether it exists and under what [`Policy`]:
//! who may call it, whether a browser from another origin may, whether it
//! takes a body, and which TLS version it needs. [`Policy`] has no
//! `Default`, and its only constructors are named for what they allow, so a
//! route cannot be added without saying who may call it. The authenticated
//! constructor is the short one.
//!
//! Dispatch is exact match on the raw path — no parameters, no wildcards, no
//! prefix handlers, no percent-decoding, no normalisation. Anything not in
//! the table is absent. Under `/api/v1` "absent" is answered with 401,
//! because the whole namespace outside the pairing surface requires a device
//! (criterion 26) and an unauthenticated caller must not learn which paths
//! exist. `/api/<anything else>` is an unsupported version and never reaches
//! a v1 handler.

use http::Method;

/// Who may call a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// Anyone who reaches Core with a valid `Host`.
    None,
    /// A paired device. M1D implements no authentication, so every request
    /// to such a route is refused with 401; M1E adds verification.
    Device,
}

/// Whether a browser on another origin may call a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    /// Any `Origin` header, and any cross-site fetch metadata, is refused.
    Deny,
    /// A browser request is allowed only from Core's own origin.
    SameOrigin,
}

/// What a route accepts as a request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Nothing. A request with a body is refused unread.
    None,
    /// JSON of at most `limit` bytes, parsed strictly.
    Json {
        /// Largest body, checked before reading.
        limit: usize,
    },
}

/// Which TLS version a route needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tls {
    /// The API floor, TLS 1.2 with extended master secret.
    Any,
    /// TLS 1.3 only: the pairing routes (ADR-003 §4).
    Tls13,
}

/// A route's policy in one mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    auth: Auth,
    browser: Browser,
    body: Body,
    tls: Tls,
}

impl Policy {
    /// A paired device, no cross-origin browser, no body, TLS floor. The
    /// default a new route should start from.
    #[must_use]
    pub const fn device() -> Self {
        Self {
            auth: Auth::Device,
            browser: Browser::Deny,
            body: Body::None,
            tls: Tls::Any,
        }
    }

    /// **Unauthenticated.** Every use is visible in the table and pinned by
    /// the enumeration test.
    #[must_use]
    pub const fn public(browser: Browser, body: Body, tls: Tls) -> Self {
        Self {
            auth: Auth::None,
            browser,
            body,
            tls,
        }
    }

    /// Who may call.
    #[must_use]
    pub fn auth(self) -> Auth {
        self.auth
    }

    /// Browser policy.
    #[must_use]
    pub fn browser(self) -> Browser {
        self.browser
    }

    /// Body policy.
    #[must_use]
    pub fn body(self) -> Body {
        self.body
    }

    /// TLS policy.
    #[must_use]
    pub fn tls(self) -> Tls {
        self.tls
    }
}

/// The handlers that exist. A closed set; the test-only ones exist only in
/// test builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// `/healthz`.
    Healthz,
    /// `/`: a static page saying Core is running.
    Index,
    /// `/api/v1/system/diagnostics`: in recovery, the redacted payload; in
    /// normal mode a device route whose handler arrives in M1F.
    SystemDiagnostics,
    /// Test only: reports a digest of the connection's channel binding.
    #[cfg(test)]
    TestBinding,
    /// Test only: parses a strict JSON body.
    #[cfg(test)]
    TestJson,
}

/// One route.
#[derive(Debug, Clone)]
pub struct Route {
    /// The method.
    pub method: Method,
    /// The exact path.
    pub path: &'static str,
    /// The handler.
    pub endpoint: Endpoint,
    /// Its policy in normal mode; `None` means absent.
    pub normal: Option<Policy>,
    /// Its policy in recovery mode; `None` means `503 internal.recovery_mode`.
    pub recovery: Option<Policy>,
}

/// M1D's routes. Everything else in `M1-IMPLEMENTATION-PLAN.md` §8 arrives
/// with the pass that implements it; nothing is declared ahead of its
/// handler except the diagnostics route, whose recovery form M1B already
/// specified and whose normal form must exist so the recovery routes stay a
/// subset of the normal ones.
pub const ROUTES: &[Route] = &[
    Route {
        method: Method::GET,
        path: "/healthz",
        endpoint: Endpoint::Healthz,
        normal: Some(Policy::public(Browser::Deny, Body::None, Tls::Any)),
        recovery: Some(Policy::public(Browser::Deny, Body::None, Tls::Any)),
    },
    Route {
        method: Method::GET,
        path: "/",
        endpoint: Endpoint::Index,
        normal: Some(Policy::public(Browser::SameOrigin, Body::None, Tls::Any)),
        recovery: None,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/system/diagnostics",
        endpoint: Endpoint::SystemDiagnostics,
        normal: Some(Policy::device()),
        // Unauthenticated only because recovery cannot authenticate, and
        // therefore cut down to the allowlisted payload (plan §4.6).
        recovery: Some(Policy::public(Browser::Deny, Body::None, Tls::Any)),
    },
];

/// Which mode Core is serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServingMode {
    /// Normal.
    Normal,
    /// Recovery.
    Recovery,
}

/// Where an unmatched path falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    /// `/api/v1` or below.
    ApiV1,
    /// `/api` or `/api/<something other than v1>`.
    ApiOther,
    /// Anything else.
    Other,
}

/// The result of looking a request up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A route, and its policy in the current mode.
    Found(Endpoint, Policy),
    /// The path exists in this mode, but not for this method. `device` is
    /// true when every route at the path requires a device.
    WrongMethod {
        /// Every route at the path requires a device.
        device: bool,
        /// The methods the path does accept in this mode.
        allow: Vec<Method>,
    },
    /// Nothing at this path in this mode.
    Absent(Namespace),
}

fn namespace(path: &str) -> Namespace {
    if path == "/api/v1" || path.starts_with("/api/v1/") {
        Namespace::ApiV1
    } else if path == "/api" || path.starts_with("/api/") {
        Namespace::ApiOther
    } else {
        Namespace::Other
    }
}

/// Looks `method path` up in `routes` for `mode`.
#[must_use]
pub fn lookup(routes: &[Route], mode: ServingMode, method: &Method, path: &str) -> Lookup {
    let policy = |route: &Route| match mode {
        ServingMode::Normal => route.normal,
        ServingMode::Recovery => route.recovery,
    };
    let at_path: Vec<(&Route, Policy)> = routes
        .iter()
        .filter(|route| route.path == path)
        .filter_map(|route| policy(route).map(|p| (route, p)))
        .collect();
    if let Some((route, found)) = at_path.iter().find(|(route, _)| route.method == *method) {
        return Lookup::Found(route.endpoint, *found);
    }
    if at_path.is_empty() {
        return Lookup::Absent(namespace(path));
    }
    Lookup::WrongMethod {
        device: at_path.iter().all(|(_, p)| p.auth() == Auth::Device),
        allow: at_path
            .iter()
            .map(|(route, _)| route.method.clone())
            .collect(),
    }
}

#[cfg(test)]
pub(crate) const TEST_ROUTES: &[Route] = &[
    Route {
        method: Method::GET,
        path: "/healthz",
        endpoint: Endpoint::Healthz,
        normal: Some(Policy::public(Browser::Deny, Body::None, Tls::Any)),
        recovery: Some(Policy::public(Browser::Deny, Body::None, Tls::Any)),
    },
    Route {
        method: Method::GET,
        path: "/test/binding",
        endpoint: Endpoint::TestBinding,
        normal: Some(Policy::public(Browser::Deny, Body::None, Tls::Tls13)),
        recovery: None,
    },
    Route {
        method: Method::POST,
        path: "/test/json",
        endpoint: Endpoint::TestJson,
        normal: Some(Policy::public(
            Browser::Deny,
            Body::Json { limit: 64 },
            Tls::Any,
        )),
        recovery: None,
    },
    Route {
        method: Method::GET,
        path: "/test/device",
        endpoint: Endpoint::SystemDiagnostics,
        normal: Some(Policy::device()),
        recovery: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(route: &Route) -> String {
        let mode = |policy: Option<Policy>| match policy {
            None => "absent".to_owned(),
            Some(p) => format!("{:?}/{:?}/{:?}/{:?}", p.auth, p.browser, p.body, p.tls),
        };
        format!(
            "{} {} normal={} recovery={}",
            route.method,
            route.path,
            mode(route.normal),
            mode(route.recovery)
        )
    }

    /// Criterion 37 and 46 (route half). Adding, removing or loosening a
    /// route fails this test until the literal list is updated, which is a
    /// reviewed change.
    #[test]
    fn route_table_matches_expected_literal_list() {
        let actual: Vec<String> = ROUTES.iter().map(describe).collect();
        assert_eq!(
            actual,
            [
                "GET /healthz normal=None/Deny/None/Any recovery=None/Deny/None/Any",
                "GET / normal=None/SameOrigin/None/Any recovery=absent",
                "GET /api/v1/system/diagnostics normal=Device/Deny/None/Any \
                 recovery=None/Deny/None/Any",
            ]
        );
    }

    #[test]
    fn no_route_takes_a_parameter_a_command_or_a_runtime_api() {
        for route in ROUTES {
            assert!(route.path.starts_with('/'));
            assert!(
                !route.path.contains(['{', '}', '*', ':', '%', '?']),
                "{} is a literal path",
                route.path
            );
            for word in [
                "docker",
                "podman",
                "container",
                "exec",
                "shell",
                "command",
                "unit",
            ] {
                assert!(!route.path.contains(word), "{} names {word}", route.path);
            }
        }
    }

    #[test]
    fn every_recovery_route_is_also_a_normal_route() {
        for route in ROUTES.iter().filter(|r| r.recovery.is_some()) {
            assert!(
                route.normal.is_some(),
                "{} exists only in recovery",
                route.path
            );
        }
    }

    #[test]
    fn recovery_has_no_state_changing_route_and_no_pairing_route() {
        for route in ROUTES.iter().filter(|r| r.recovery.is_some()) {
            assert_eq!(route.method, Method::GET, "{}", route.path);
            assert!(!route.path.starts_with("/api/v1/pair"), "{}", route.path);
            let policy = route.recovery.expect("recovery");
            assert_eq!(policy.body(), Body::None);
        }
    }

    #[test]
    fn nothing_but_get_exists_in_m1d() {
        assert!(ROUTES.iter().all(|r| r.method == Method::GET));
        assert!(ROUTES
            .iter()
            .flat_map(|r| [r.normal, r.recovery])
            .flatten()
            .all(|p| p.body() == Body::None));
    }

    #[test]
    fn lookup_is_exact() {
        let normal = ServingMode::Normal;
        assert!(matches!(
            lookup(ROUTES, normal, &Method::GET, "/healthz"),
            Lookup::Found(Endpoint::Healthz, _)
        ));
        for path in [
            "/healthz/",
            "/HEALTHZ",
            "/healthz%2F",
            "//healthz",
            "/./healthz",
            "/healthz?x",
        ] {
            assert!(
                matches!(
                    lookup(ROUTES, normal, &Method::GET, path),
                    Lookup::Absent(_)
                ),
                "{path}"
            );
        }
        assert_eq!(
            lookup(ROUTES, normal, &Method::POST, "/healthz"),
            Lookup::WrongMethod {
                device: false,
                allow: vec![Method::GET]
            }
        );
        assert!(matches!(
            lookup(
                ROUTES,
                normal,
                &Method::DELETE,
                "/api/v1/system/diagnostics"
            ),
            Lookup::WrongMethod { device: true, .. }
        ));
    }

    #[test]
    fn unknown_versions_never_fall_into_v1() {
        let normal = ServingMode::Normal;
        for (path, expected) in [
            ("/api/v1/nothing", Namespace::ApiV1),
            ("/api/v1", Namespace::ApiV1),
            ("/api/v2/system/diagnostics", Namespace::ApiOther),
            ("/api/V1/system/diagnostics", Namespace::ApiOther),
            ("/api/v1.1/system/diagnostics", Namespace::ApiOther),
            ("/api/%76%31/system/diagnostics", Namespace::ApiOther),
            ("/api", Namespace::ApiOther),
            ("/apiv1/system/diagnostics", Namespace::Other),
            ("/v1/system/diagnostics", Namespace::Other),
        ] {
            assert_eq!(
                lookup(ROUTES, normal, &Method::GET, path),
                Lookup::Absent(expected),
                "{path}"
            );
        }
    }

    #[test]
    fn recovery_serves_only_its_subset() {
        let recovery = ServingMode::Recovery;
        assert!(matches!(
            lookup(ROUTES, recovery, &Method::GET, "/"),
            Lookup::Absent(Namespace::Other)
        ));
        assert!(matches!(
            lookup(ROUTES, recovery, &Method::GET, "/api/v1/system/diagnostics"),
            Lookup::Found(Endpoint::SystemDiagnostics, p) if p.auth() == Auth::None
        ));
    }
}
