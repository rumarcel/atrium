//! The route table: every route, and its policy, in one literal list.
//!
//! A route states, per mode, whether it exists and under what [`Policy`]:
//! who may call it, whether a browser from another origin may, whether it
//! takes a body, and which TLS version it needs. [`Policy`] has no
//! `Default`, and its only constructors are named for what they allow, so a
//! route cannot be added without saying who may call it. The authenticated
//! constructor is the short one.
//!
//! Dispatch is exact match on the raw path — no wildcards, no prefix
//! handlers, no percent-decoding, no normalisation. The one parameter is
//! `{deviceId}` (M1E), which matches a whole segment of exactly 32 lowercase
//! hex characters and nothing else; a segment that is not one makes the path
//! absent. Anything not in the table is absent. Under `/api/v1` "absent" is answered with 401,
//! because the whole namespace outside the pairing surface requires a device
//! (criterion 26) and an unauthenticated caller must not learn which paths
//! exist. `/api/<anything else>` is an unsupported version and never reaches
//! a v1 handler.

use atrium_pairing::wire::Id;
use http::Method;

/// Who may call a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// Anyone who reaches Core with a valid `Host`.
    None,
    /// A paired device, by `Authorization: Bearer` (M1E).
    Device,
    /// **Unauthenticated**, and one of the three pairing routes: TLS 1.3
    /// only, no browser, rate limited per source before the body is read.
    Pairing,
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

    /// A pairing route: unauthenticated, rate limited, TLS 1.3, no browser.
    #[must_use]
    pub const fn pairing(body: Body) -> Self {
        Self {
            auth: Auth::Pairing,
            browser: Browser::Deny,
            body,
            tls: Tls::Tls13,
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
    /// `GET /api/v1/pair/info`.
    PairInfo,
    /// `POST /api/v1/pair/begin`.
    PairBegin,
    /// `POST /api/v1/pair/complete`.
    PairComplete,
    /// `GET /api/v1/me`.
    Me,
    /// `GET /api/v1/devices`.
    Devices,
    /// `DELETE /api/v1/devices/{deviceId}`.
    RevokeDevice,
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

/// Largest `pair/begin` body: a 64-character name is at most 128 bytes of
/// UTF-8, or six times that as JSON `\u` escapes; the rest is fixed.
pub const BEGIN_BODY_LIMIT: usize = 1024;
/// Largest `pair/complete` body: two fixed-length fields.
pub const COMPLETE_BODY_LIMIT: usize = 256;

/// The routes through M1E. Everything else in `M1-IMPLEMENTATION-PLAN.md` §8
/// arrives with the pass that implements it; nothing is declared ahead of
/// its handler except the diagnostics route, whose recovery form M1B already
/// specified and whose normal form must exist so the recovery routes stay a
/// subset of the normal ones. `POST /api/v1/devices/actions/revoke-all`
/// needs the confirmation mechanism and is not here.
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
    // Pairing: never in recovery (plan §4.6).
    Route {
        method: Method::GET,
        path: "/api/v1/pair/info",
        endpoint: Endpoint::PairInfo,
        normal: Some(Policy::pairing(Body::None)),
        recovery: None,
    },
    Route {
        method: Method::POST,
        path: "/api/v1/pair/begin",
        endpoint: Endpoint::PairBegin,
        normal: Some(Policy::pairing(Body::Json {
            limit: BEGIN_BODY_LIMIT,
        })),
        recovery: None,
    },
    Route {
        method: Method::POST,
        path: "/api/v1/pair/complete",
        endpoint: Endpoint::PairComplete,
        normal: Some(Policy::pairing(Body::Json {
            limit: COMPLETE_BODY_LIMIT,
        })),
        recovery: None,
    },
    // Devices: recovery cannot authenticate, so none of these exist there.
    Route {
        method: Method::GET,
        path: "/api/v1/me",
        endpoint: Endpoint::Me,
        normal: Some(Policy::device()),
        recovery: None,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/devices",
        endpoint: Endpoint::Devices,
        normal: Some(Policy::device()),
        recovery: None,
    },
    Route {
        method: Method::DELETE,
        path: "/api/v1/devices/{deviceId}",
        endpoint: Endpoint::RevokeDevice,
        normal: Some(Policy::device()),
        recovery: None,
    },
];

/// The only path parameter.
const DEVICE_ID: &str = "{deviceId}";

/// Matches `path` against a route's `pattern`: literally, or with
/// `{deviceId}` standing for exactly one 32-lowercase-hex segment.
fn match_path(pattern: &str, path: &str) -> Option<Option<Id>> {
    if !pattern.contains('{') {
        return (pattern == path).then_some(None);
    }
    let mut found = None;
    let mut expected = pattern.split('/');
    let mut actual = path.split('/');
    loop {
        match (expected.next(), actual.next()) {
            (None, None) => return Some(found),
            (Some(DEVICE_ID), Some(segment)) => found = Some(Id::parse(segment)?),
            (Some(literal), Some(segment)) if literal == segment => {}
            _ => return None,
        }
    }
}

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
    /// A route, its policy in the current mode, and the `{deviceId}` in the
    /// path if the route has one.
    Found(Endpoint, Policy, Option<Id>),
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
    let at_path: Vec<(&Route, Policy, Option<Id>)> = routes
        .iter()
        .filter_map(|route| {
            let parameter = match_path(route.path, path)?;
            policy(route).map(|p| (route, p, parameter))
        })
        .collect();
    if let Some((route, found, parameter)) =
        at_path.iter().find(|(route, _, _)| route.method == *method)
    {
        return Lookup::Found(route.endpoint, *found, *parameter);
    }
    if at_path.is_empty() {
        return Lookup::Absent(namespace(path));
    }
    Lookup::WrongMethod {
        device: at_path.iter().all(|(_, p, _)| p.auth() == Auth::Device),
        allow: at_path
            .iter()
            .map(|(route, _, _)| route.method.clone())
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
                "GET /api/v1/pair/info normal=Pairing/Deny/None/Tls13 recovery=absent",
                "POST /api/v1/pair/begin normal=Pairing/Deny/Json { limit: 1024 }/Tls13 \
                 recovery=absent",
                "POST /api/v1/pair/complete normal=Pairing/Deny/Json { limit: 256 }/Tls13 \
                 recovery=absent",
                "GET /api/v1/me normal=Device/Deny/None/Any recovery=absent",
                "GET /api/v1/devices normal=Device/Deny/None/Any recovery=absent",
                "DELETE /api/v1/devices/{deviceId} normal=Device/Deny/None/Any recovery=absent",
            ]
        );
    }

    #[test]
    fn no_route_takes_a_free_parameter_a_command_or_a_runtime_api() {
        for route in ROUTES {
            assert!(route.path.starts_with('/'));
            let literal = route.path.replace(DEVICE_ID, "");
            assert!(
                !literal.contains(['{', '}', '*', ':', '%', '?']),
                "{} is a literal path apart from {DEVICE_ID}",
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

    /// Criterion 28's route half: only the two pairing POSTs take a body,
    /// each bounded, each JSON; everything else takes none.
    #[test]
    fn only_the_pairing_posts_take_a_body() {
        for route in ROUTES {
            for policy in [route.normal, route.recovery].into_iter().flatten() {
                match route.endpoint {
                    Endpoint::PairBegin | Endpoint::PairComplete => {
                        assert_eq!(route.method, Method::POST);
                        assert!(
                            matches!(policy.body(), Body::Json { limit } if limit <= 1024),
                            "{}",
                            route.path
                        );
                    }
                    _ => {
                        assert_ne!(route.method, Method::POST, "{}", route.path);
                        assert_eq!(policy.body(), Body::None, "{}", route.path);
                    }
                }
            }
        }
    }

    #[test]
    fn every_unauthenticated_route_is_named_and_pairing_is_tls13_only() {
        let open: Vec<&str> = ROUTES
            .iter()
            .filter(|r| r.normal.is_some_and(|p| p.auth() != Auth::Device))
            .map(|r| r.path)
            .collect();
        assert_eq!(
            open,
            [
                "/healthz",
                "/",
                "/api/v1/pair/info",
                "/api/v1/pair/begin",
                "/api/v1/pair/complete"
            ]
        );
        for route in ROUTES
            .iter()
            .filter(|r| r.path.starts_with("/api/v1/pair/"))
        {
            let policy = route.normal.expect("normal");
            assert_eq!(policy.auth(), Auth::Pairing);
            assert_eq!(policy.tls(), Tls::Tls13);
            assert_eq!(policy.browser(), Browser::Deny);
            assert!(route.recovery.is_none());
        }
    }

    #[test]
    fn the_device_id_parameter_matches_one_exact_segment() {
        let normal = ServingMode::Normal;
        let id = "00112233445566778899aabbccddeeff";
        assert!(matches!(
            lookup(ROUTES, normal, &Method::DELETE, &format!("/api/v1/devices/{id}")),
            Lookup::Found(Endpoint::RevokeDevice, _, Some(found)) if found.to_hex() == id
        ));
        for path in [
            "/api/v1/devices/00112233445566778899AABBCCDDEEFF".to_owned(),
            "/api/v1/devices/00112233445566778899aabbccddeef".to_owned(),
            format!("/api/v1/devices/{id}/"),
            format!("/api/v1/devices/{id}/x"),
            format!("/api/v1/devices//{id}"),
            "/api/v1/devices/%30%30".to_owned(),
            "/api/v1/devices/{deviceId}".to_owned(),
        ] {
            assert_eq!(
                lookup(ROUTES, normal, &Method::DELETE, &path),
                Lookup::Absent(Namespace::ApiV1),
                "{path}"
            );
        }
        assert_eq!(
            lookup(
                ROUTES,
                normal,
                &Method::GET,
                &format!("/api/v1/devices/{id}")
            ),
            Lookup::WrongMethod {
                device: true,
                allow: vec![Method::DELETE]
            }
        );
    }

    #[test]
    fn lookup_is_exact() {
        let normal = ServingMode::Normal;
        assert!(matches!(
            lookup(ROUTES, normal, &Method::GET, "/healthz"),
            Lookup::Found(Endpoint::Healthz, _, None)
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
            Lookup::Found(Endpoint::SystemDiagnostics, p, None) if p.auth() == Auth::None
        ));
    }
}
