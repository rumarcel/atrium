//! M1F over the real listener: the system routes answer devices only, keep
//! every network policy, and serialize what the providers read.

use super::tests::{assert_common_headers, assert_problem, connect, exchange, get, Server};
use crate::providers::fixture::Tree;

const SYSTEM_ROUTES: [&str; 6] = [
    "/api/v1/system",
    "/api/v1/system/metrics",
    "/api/v1/system/capabilities",
    "/api/v1/network/interfaces",
    "/api/v1/storage/filesystems",
    "/api/v1/system/diagnostics",
];

fn authorized(path: &str, host: &str, token: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\r\n")
        .into_bytes()
}

#[tokio::test]
async fn every_system_route_needs_a_device_and_revocation_is_immediate() {
    let server = Server::normal().await;
    let host = server.host();
    let token = server.state().device("Desk");
    let unknown = atrium_pairing::token::DeviceToken::from_bytes([9; 32])
        .encode()
        .to_string();
    let mut stream = server.connect().await;
    for path in SYSTEM_ROUTES {
        let anonymous = exchange(&mut stream, &get(path, &host)).await;
        assert_problem(&anonymous, 401, "auth.unauthorized");
        let stranger = exchange(&mut stream, &authorized(path, &host, &unknown)).await;
        assert_problem(&stranger, 401, "auth.unauthorized");
        let device = exchange(&mut stream, &authorized(path, &host, &token)).await;
        assert_eq!(device.status, 200, "{path}: {device:?}");
        assert_common_headers(&device);
        assert_eq!(
            device.header("content-type"),
            Some("application/json; charset=utf-8")
        );
    }
    server
        .state()
        .database()
        .connection()
        .execute("DELETE FROM devices", ())
        .expect("revoke");
    for path in SYSTEM_ROUTES {
        let revoked = exchange(&mut stream, &authorized(path, &host, &token)).await;
        assert_problem(&revoked, 401, "auth.unauthorized");
    }
    server.stop().await;
}

/// Provider code never runs before authorization: over an empty host tree
/// every provider read records a problem, so an unauthenticated sweep that
/// leaves the record empty never reached a provider.
#[tokio::test]
async fn no_provider_runs_before_the_device_check() {
    let tree = Tree::new();
    let server = Server::normal_with_host(tree.root()).await;
    let host = server.host();
    let mut stream = server.connect().await;
    for path in SYSTEM_ROUTES {
        for request in [
            get(path, &host),
            authorized(path, &host, "AAAA"),
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\n\r\n")
                .into_bytes(),
        ] {
            let reply = exchange(&mut stream, &request).await;
            assert!(
                reply.status == 401 || reply.status == 403,
                "{path}: {reply:?}"
            );
        }
    }
    assert!(
        server.state().issues.snapshot().is_empty(),
        "a provider ran"
    );

    let token = server.state().device("Desk");
    let metrics = exchange(
        &mut stream,
        &authorized("/api/v1/system/metrics", &host, &token),
    )
    .await;
    assert_eq!(metrics.status, 200);
    assert!(!server.state().issues.snapshot().is_empty(), "now one has");
    // Degraded, not failed: every measurement null with a reason.
    let value = metrics.json();
    assert!(value["memory"]["totalBytes"].is_null());
    assert!(value["cpu"]["usagePercent"].is_null());
    assert!(value["unavailable"]
        .as_array()
        .expect("unavailable")
        .iter()
        .any(|u| u["field"] == "memory.totalBytes" && u["reason"] == "procfs_unreadable"));
    server.stop().await;
}

#[tokio::test]
async fn system_routes_keep_the_host_origin_and_tls_policy() {
    let server = Server::normal().await;
    let host = server.host();
    let token = server.state().device("Desk");
    let mut stream = server.connect().await;
    let rebinding = exchange(
        &mut stream,
        &authorized("/api/v1/system", "attacker.example", &token),
    )
    .await;
    assert_problem(&rebinding, 421, "request.host_rejected");
    let browser = format!(
        "GET /api/v1/system HTTP/1.1\r\nHost: {host}\r\nOrigin: https://{host}\r\n\
         Authorization: Bearer {token}\r\n\r\n"
    );
    assert_problem(
        &exchange(&mut stream, browser.as_bytes()).await,
        403,
        "request.origin_rejected",
    );
    let body = format!(
        "GET /api/v1/system/metrics HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\
         Content-Length: 2\r\n\r\n{{}}"
    );
    assert_problem(
        &exchange(&mut stream, body.as_bytes()).await,
        400,
        "validation.body_not_accepted",
    );
    let wrong_method = format!(
        "DELETE /api/v1/system HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    let reply = exchange(&mut stream, wrong_method.as_bytes()).await;
    assert_problem(&reply, 405, "request.method_not_allowed");
    assert_eq!(reply.header("allow"), Some("GET"));
    // The API floor, TLS 1.2 with EMS, still applies to reads.
    let mut tls12 = connect(server.port, server.pin, &[&rustls::version::TLS12]).await;
    let reply = exchange(&mut tls12, &authorized("/api/v1/system", &host, &token)).await;
    assert_eq!(reply.status, 200);
    server.stop().await;
}

#[tokio::test]
async fn recovery_serves_none_of_the_system_domain_but_its_diagnostics() {
    let server = Server::recovery().await;
    let host = server.host();
    let mut stream = server.connect().await;
    for path in &SYSTEM_ROUTES[..5] {
        assert_problem(
            &exchange(&mut stream, &get(path, &host)).await,
            503,
            "internal.recovery_mode",
        );
    }
    let diagnostics = exchange(&mut stream, &get("/api/v1/system/diagnostics", &host)).await;
    assert_eq!(diagnostics.status, 200);
    assert_eq!(diagnostics.json()["state"], "recovery");
    assert!(diagnostics.json().get("capabilities").is_none());
    server.stop().await;
}

/// The real host, serialized: the shapes the API documents, with values
/// the kernel gave and reasons where it gave none.
#[tokio::test]
async fn real_host_responses_have_the_documented_shape() {
    let server = Server::normal().await;
    let host = server.host();
    let token = server.state().device("Desk");
    let mut stream = server.connect().await;
    let keys = |value: &serde_json::Value| -> Vec<String> {
        value.as_object().expect("object").keys().cloned().collect()
    };

    let request = authorized("/api/v1/system", &host, &token);
    let system = exchange(&mut stream, &request).await.json();
    assert_eq!(
        keys(&system),
        [
            "arch",
            "bootId",
            "coreVersion",
            "hostname",
            "kernel",
            "os",
            "serverId",
            "unavailable",
            "uptimeSeconds"
        ]
    );
    assert_eq!(system["kernel"]["name"], "Linux");
    assert_eq!(system["serverId"], server.identity.server_id().to_string());

    let request = authorized("/api/v1/system/metrics", &host, &token);
    let metrics = exchange(&mut stream, &request).await.json();
    assert_eq!(
        keys(&metrics),
        [
            "at",
            "cpu",
            "load",
            "memory",
            "swap",
            "temperatures",
            "unavailable"
        ]
    );
    // The harness runs no sampler: pending, never 0.
    assert!(metrics["cpu"]["usagePercent"].is_null());
    assert!(metrics["unavailable"]
        .as_array()
        .expect("list")
        .iter()
        .any(|u| u["field"] == "cpu.usagePercent" && u["reason"] == "first_sample_pending"));
    assert!(metrics["memory"]["totalBytes"]
        .as_u64()
        .is_some_and(|b| b > 0));

    let request = authorized("/api/v1/network/interfaces", &host, &token);
    let interfaces = exchange(&mut stream, &request).await.json();
    assert_eq!(keys(&interfaces), ["interfaces", "unavailable"]);
    let list = interfaces["interfaces"].as_array().expect("a list");
    assert!(list.iter().any(|i| i["name"] == "lo"));
    for interface in list {
        assert!(interface.get("primary").is_none());
        assert!(interface["addresses"].is_array());
    }

    let request = authorized("/api/v1/storage/filesystems", &host, &token);
    let storage = exchange(&mut stream, &request).await.json();
    assert_eq!(keys(&storage), ["filesystems", "unavailable"]);

    let request = authorized("/api/v1/system/capabilities", &host, &token);
    let capabilities = exchange(&mut stream, &request).await.json();
    assert_eq!(
        keys(&capabilities),
        [
            "container",
            "discovery",
            "hardware",
            "network",
            "packages",
            "privileged",
            "services",
            "storage"
        ]
    );
    assert_eq!(capabilities["hardware"]["available"], true);
    assert_eq!(capabilities["container"]["via"], "agent");

    let request = authorized("/api/v1/system/diagnostics", &host, &token);
    let diagnostics = exchange(&mut stream, &request).await.json();
    assert_eq!(diagnostics["state"], "normal");
    let text = diagnostics.to_string();
    for leak in [token.as_str(), "127.0.0.1", "Desk"] {
        assert!(!text.contains(leak), "{leak} in diagnostics");
    }
    server.stop().await;
}
