//! Read-only discovery over the existing enrolled agent connection. The
//! desktop receives no container socket, image repository, environment, labels
//! or command line. Published ports are suggestions, never probed on import.
use crate::{
    server_control::ServerControl,
    service_discovery::{ServiceDiscoveryCandidate, ServiceDiscoveryResponse},
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use tauri::Webview;

const MAX_CONTAINERS: usize = 64;
const MAX_PORTS: usize = 8;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const APPLICATIONS: &[&str] = &[
    "jellyfin",
    "plex",
    "emby",
    "radarr",
    "sonarr",
    "bazarr",
    "prowlarr",
    "lidarr",
    "readarr",
    "immich",
    "navidrome",
    "audiobookshelf",
    "qbittorrent",
    "sabnzbd",
    "transmission",
    "metube",
    "homarr",
    "home-assistant",
    "nextcloud",
    "portainer",
    "glances",
    "grafana",
    "pihole",
    "adguard-home",
    "crafty",
    "gerbera",
];

// Explicit null is part of the wire contract; an omitted optional field must
// not silently turn into a different, accepted version of the schema.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
enum RuntimeKind {
    Docker,
    Podman,
}

impl RuntimeKind {
    fn key(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Podman => "Podman",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum SourceState {
    Ready,
    Missing,
    Stale,
    Unavailable,
    Invalid,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ContainerState {
    Running,
    Stopped,
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InventoryDocument {
    version: u8,
    target: String,
    sources: Vec<InventorySource>,
    maintenance: Maintenance,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InventorySource {
    runtime: RuntimeKind,
    scope: String,
    state: SourceState,
    #[serde(deserialize_with = "nullable")]
    age_seconds: Option<u64>,
    skipped_count: usize,
    containers: Vec<Container>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Container {
    id: String,
    name: String,
    #[serde(deserialize_with = "nullable")]
    application: Option<String>,
    state: ContainerState,
    ports: Vec<PublishedPort>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedPort {
    host_port: u16,
    container_port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Maintenance {
    // Absence of the Linux reboot-required marker is not evidence that the OS
    // is up-to-date, or that stopping active workloads would be safe.
    #[serde(deserialize_with = "nullable")]
    reboot_required: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    runtime: RuntimeKind,
    scope: &'static str,
    state: SourceState,
    age_seconds: Option<u64>,
    skipped_count: usize,
    container_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInventoryDiscovery {
    /// The enrolled address every suggested URL points at. The frontend
    /// re-checks each candidate against it instead of a built-in host.
    address: String,
    discovery: ServiceDiscoveryResponse,
    sources: Vec<SourceSummary>,
    maintenance: Maintenance,
}

#[tauri::command]
pub async fn discover_server_inventory(
    caller: Webview,
    control: tauri::State<'_, ServerControl>,
) -> Result<ServerInventoryDiscovery, String> {
    authorize_main(caller.label())?;
    // The enrolled address is the only host this discovery may describe: the
    // snapshot must have been written by that server, and every suggested URL
    // points back at it rather than at any host named inside the document.
    let address = control.enrolled_address()?;
    let document: InventoryDocument = control.read_inventory().await?;
    create_discovery(document, &address)
}

fn authorize_main(label: &str) -> Result<(), String> {
    if label != "main" {
        return Err("Server inventory is available only to the trusted main UI.".into());
    }
    Ok(())
}

impl InventoryDocument {
    fn validate(&self, address: &str) -> Result<(), String> {
        let invalid = || "The server inventory has an unsupported or unsafe schema.".to_string();
        if self.version != 1
            || self.target != address
            || self.sources.len() != 2
            || self.maintenance.reboot_required == Some(false)
        {
            return Err(invalid());
        }
        let mut runtimes = HashSet::new();
        for source in &self.sources {
            if !runtimes.insert(source.runtime)
                || source.scope != "rootful"
                || source.containers.len() > MAX_CONTAINERS
                || source.skipped_count > 50_000
                || source.age_seconds.is_some_and(|age| age > MAX_SAFE_INTEGER)
            {
                return Err(invalid());
            }
            if source.state == SourceState::Ready {
                if source.age_seconds.is_none_or(|age| age > 180) {
                    return Err(invalid());
                }
            } else if !source.containers.is_empty() {
                // Stale data remains informative, never eligible for import.
                return Err(invalid());
            }
            let mut ids = HashSet::new();
            for container in &source.containers {
                if container.id.len() != 12
                    || !container
                        .id
                        .bytes()
                        .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
                    || !ids.insert(&container.id)
                    || container.name.is_empty()
                    || container.name.chars().count() > 80
                    || container.name.trim() != container.name
                    || container.name.chars().any(char::is_control)
                    || container
                        .application
                        .as_deref()
                        .is_some_and(|app| !APPLICATIONS.contains(&app))
                    || container.ports.len() > MAX_PORTS
                {
                    return Err(invalid());
                }
                let mut ports = HashSet::new();
                if container.ports.iter().any(|port| {
                    port.host_port == 0 || port.container_port == 0 || !ports.insert(port)
                }) {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

fn create_discovery(
    document: InventoryDocument,
    address: &str,
) -> Result<ServerInventoryDiscovery, String> {
    document.validate(address)?;
    let mut candidates = Vec::new();
    let mut sources = Vec::new();
    let mut skipped_count = 0;
    for source in document.sources {
        skipped_count += source.skipped_count;
        sources.push(SourceSummary {
            runtime: source.runtime,
            scope: "rootful",
            state: source.state,
            age_seconds: source.age_seconds,
            skipped_count: source.skipped_count,
            container_count: source.containers.len(),
        });
        for container in source.containers {
            let endpoint = suggested_endpoint(&container, address);
            let description = endpoint
                .as_ref()
                .map(|(_, port)| {
                    format!(
                        "{} · {} → {}",
                        source.runtime.label(),
                        port.container_port,
                        port.host_port
                    )
                })
                .unwrap_or_else(|| source.runtime.label().to_string());
            let url = endpoint.map(|(url, _)| url);
            candidates.push(ServiceDiscoveryCandidate {
                source_id: format!("{}:{}", source.runtime.key(), container.id),
                name: container.name,
                description: Some(description),
                url,
                icon_hint: Some(
                    container
                        .application
                        .unwrap_or_else(|| source.runtime.key().into()),
                ),
            });
        }
    }
    Ok(ServerInventoryDiscovery {
        address: address.to_string(),
        discovery: ServiceDiscoveryResponse {
            source: "server-agent",
            source_service_id: "server-agent".into(),
            candidates,
            skipped_count,
        },
        sources,
        maintenance: document.maintenance,
    })
}

fn suggested_endpoint<'a>(
    container: &'a Container,
    address: &str,
) -> Option<(String, &'a PublishedPort)> {
    if container.state != ContainerState::Running {
        return None;
    }
    let app = container.application.as_deref()?;
    // Conservative known web ports only. Image identification and published
    // ports are hints, not service health or a claim of authenticated access.
    // Do not label an arbitrary TCP port (SSH/database/game traffic) as HTTP.
    let web_ports: &[(u16, &str)] = match app {
        "jellyfin" => &[(8096, "http"), (8920, "https")],
        "radarr" => &[(7878, "http")],
        "sonarr" => &[(8989, "http")],
        "bazarr" => &[(6767, "http")],
        "qbittorrent" => &[(8080, "http")],
        "homarr" => &[(7575, "http")],
        "metube" => &[(8081, "http")],
        "portainer" => &[(9443, "https"), (9000, "http")],
        "glances" => &[(61208, "http")],
        // Other image families still receive an icon and a manual-review item;
        // avoid guessing their deployment's protocol or reverse-proxy path.
        _ => &[],
    };
    for (container_port, scheme) in web_ports {
        if let Some(port) = container
            .ports
            .iter()
            .filter(|port| port.container_port == *container_port)
            .min_by_key(|port| port.host_port)
        {
            // Keep the explicit host port in the DTO, including 80/443. Do not
            // inherit any arbitrary hostname, path, query or URL credentials.
            return Some((format!("{scheme}://{address}:{}/", port.host_port), port));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const ENROLLED: &str = "10.0.0.10";

    fn fixture() -> Value {
        json!({"version":1,"target":ENROLLED,"sources":[
            {"runtime":"docker","scope":"rootful","state":"ready","ageSeconds":4,"skippedCount":0,"containers":[
                {"id":"abcdef123456","name":"Jellyfin","application":"jellyfin","state":"running","ports":[{"hostPort":18096,"containerPort":8096}]}
            ]},
            {"runtime":"podman","scope":"rootful","state":"missing","ageSeconds":null,"skippedCount":0,"containers":[]}
        ],"maintenance":{"rebootRequired":null}})
    }

    fn parse(value: Value) -> Result<ServerInventoryDiscovery, String> {
        let document =
            serde_json::from_value(value).map_err(|_| "Invalid JSON schema".to_string())?;
        create_discovery(document, ENROLLED)
    }

    #[test]
    fn server_inventory_uses_only_fixed_target_and_published_web_ports() {
        let result = parse(fixture()).unwrap();
        assert_eq!(
            result.discovery.candidates[0].url.as_deref(),
            Some("http://10.0.0.10:18096/")
        );
        assert_eq!(
            result.discovery.candidates[0].icon_hint.as_deref(),
            Some("jellyfin")
        );
        assert_eq!(result.sources[1].state, SourceState::Missing);
        let mut value = fixture();
        value["sources"][0]["containers"][0]["ports"][0]["hostPort"] = json!(80);
        assert_eq!(
            parse(value).unwrap().discovery.candidates[0].url.as_deref(),
            Some("http://10.0.0.10:80/")
        );
        assert!(authorize_main("main").is_ok());
        assert!(authorize_main("widget-server").is_err());
        assert!(authorize_main("service-homarr").is_err());
    }

    #[test]
    fn server_inventory_never_guesses_unknown_or_stopped_services() {
        for patch in [
            json!({"application":null}),
            json!({"state":"stopped"}),
            json!({"ports":[{"hostPort":22,"containerPort":22}]}),
            json!({"ports":[]}),
        ] {
            let mut value = fixture();
            for (key, field) in patch.as_object().unwrap() {
                value["sources"][0]["containers"][0][key] = field.clone();
            }
            assert!(parse(value).unwrap().discovery.candidates[0].url.is_none());
        }
    }

    #[test]
    fn server_inventory_rejects_stale_candidates_wrong_target_and_extra_metadata() {
        let mut value = fixture();
        value["target"] = json!("127.0.0.1");
        assert!(parse(value).is_err());
        let mut value = fixture();
        value["sources"][0]["state"] = json!("stale");
        assert!(parse(value).is_err());
        let mut value = fixture();
        value["sources"][0]["ageSeconds"] = json!(181);
        assert!(parse(value).is_err());
        for extra in ["environment", "labels", "command", "mounts", "url"] {
            let mut value = fixture();
            value["sources"][0]["containers"][0][extra] = json!("must not cross trust boundary");
            assert!(parse(value).is_err());
        }
    }

    #[test]
    fn server_inventory_missing_optional_fields_and_false_reassurance_fail_closed() {
        let mut value = fixture();
        value["maintenance"]["rebootRequired"] = json!(false);
        assert!(parse(value).is_err());
        let mut value = fixture();
        value["sources"][0]
            .as_object_mut()
            .unwrap()
            .remove("ageSeconds");
        assert!(parse(value).is_err());
        let mut value = fixture();
        value["sources"][1] = value["sources"][0].clone();
        assert!(parse(value).is_err());
    }
}
