use crate::service_webviews::{ServiceCatalog, TrustedServiceEndpoint};
use reqwest::{redirect::Policy, Client, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error as StdError,
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const GLANCES_SERVICE_ID: &str = "glances";
const GLANCES_TIMEOUT: Duration = Duration::from_millis(2_500);
const MAX_PLUGIN_RESPONSE_BYTES: usize = 1_048_576;
const GLANCES_USER_AGENT: &str = "PersonalHub/0.1 monitoring";

pub struct GlancesClients {
    strict: Client,
    relaxed_local: Client,
    api_version: AtomicU8,
}

impl GlancesClients {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            strict: build_client(false)?,
            relaxed_local: build_client(true)?,
            api_version: AtomicU8::new(0),
        })
    }

    fn select(&self, target: &TrustedServiceEndpoint) -> Client {
        if target.allow_invalid_local_certificate {
            self.relaxed_local.clone()
        } else {
            self.strict.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MonitoringStatus {
    Online,
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetric {
    name: String,
    mount_point: String,
    used_bytes: u64,
    total_bytes: u64,
    percent: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerMetrics {
    status: MonitoringStatus,
    sampled_at: u64,
    cpu_percent: Option<f64>,
    memory_percent: Option<f64>,
    memory_used_bytes: Option<u64>,
    memory_total_bytes: Option<u64>,
    cpu_temperature_c: Option<f64>,
    network_download_bytes_per_second: Option<f64>,
    network_upload_bytes_per_second: Option<f64>,
    uptime_seconds: Option<u64>,
    load_average_1m: Option<f64>,
    load_average_5m: Option<f64>,
    load_average_15m: Option<f64>,
    disks: Vec<DiskMetric>,
    message: Option<String>,
}

impl ServerMetrics {
    fn unavailable(sampled_at: u64, message: impl Into<String>) -> Self {
        Self {
            status: MonitoringStatus::Unavailable,
            sampled_at,
            cpu_percent: None,
            memory_percent: None,
            memory_used_bytes: None,
            memory_total_bytes: None,
            cpu_temperature_c: None,
            network_download_bytes_per_second: None,
            network_upload_bytes_per_second: None,
            uptime_seconds: None,
            load_average_1m: None,
            load_average_5m: None,
            load_average_15m: None,
            disks: Vec::new(),
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CpuStats {
    total: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct MemoryStats {
    percent: Option<f64>,
    used: Option<u64>,
    total: Option<u64>,
    available: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct NetworkStats {
    interface_name: Option<String>,
    #[serde(default)]
    is_up: Option<bool>,
    bytes_recv_rate_per_sec: Option<f64>,
    bytes_sent_rate_per_sec: Option<f64>,
    rx: Option<f64>,
    tx: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SensorStats {
    label: Option<String>,
    #[serde(rename = "type")]
    sensor_type: Option<String>,
    unit: Option<String>,
    value: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct FilesystemStats {
    device_name: Option<String>,
    mnt_point: Option<String>,
    used: Option<u64>,
    size: Option<u64>,
    percent: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UptimeStats {
    Text(String),
    Seconds { seconds: u64 },
    Number(u64),
}

#[derive(Debug, Deserialize)]
struct LoadStats {
    min1: Option<f64>,
    min5: Option<f64>,
    min15: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitoringErrorKind {
    NotFound,
    MethodNotAllowed,
    Authentication,
    Timeout,
    Tls,
    Connection,
    Http,
    InvalidData,
    TooLarge,
    Request,
}

#[derive(Debug)]
struct MonitoringError {
    kind: MonitoringErrorKind,
    message: String,
}

impl MonitoringError {
    fn new(kind: MonitoringErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn get_server_metrics(
    caller: tauri::Webview,
    catalog: tauri::State<'_, ServiceCatalog>,
    clients: tauri::State<'_, GlancesClients>,
) -> Result<ServerMetrics, String> {
    if caller.label() != "main" {
        return Err("Monitoring is available only to the trusted Personal Hub UI.".into());
    }

    let sampled_at = unix_time_ms();
    let target = match catalog.resolve_endpoint(GLANCES_SERVICE_ID) {
        Ok(target) => target,
        Err(error) => return Ok(ServerMetrics::unavailable(sampled_at, error)),
    };
    let client = clients.select(&target);
    let cached_version = clients.api_version.load(Ordering::Relaxed);
    let api_version = if matches!(cached_version, 3 | 4) {
        cached_version
    } else {
        match discover_api_version(&client, &target.url).await {
            Ok(version) => {
                clients.api_version.store(version, Ordering::Relaxed);
                version
            }
            Err(error) => return Ok(ServerMetrics::unavailable(sampled_at, error.message)),
        }
    };

    let snapshot = fetch_snapshot(&client, &target.url, api_version, sampled_at).await;

    if snapshot.status == MonitoringStatus::Unavailable
        && snapshot.message.as_deref() == Some("The Glances API endpoint was not found.")
    {
        clients.api_version.store(0, Ordering::Relaxed);
    }

    Ok(snapshot)
}

async fn discover_api_version(client: &Client, base_url: &Url) -> Result<u8, MonitoringError> {
    match probe_endpoint(client, api_url(base_url, 4, "status")?).await {
        Ok(()) => return Ok(4),
        Err(error) if error.kind == MonitoringErrorKind::NotFound => {}
        Err(error) if error.kind == MonitoringErrorKind::MethodNotAllowed => {
            match probe_endpoint(client, api_url(base_url, 4, "version")?).await {
                Ok(()) => return Ok(4),
                Err(version_error) if version_error.kind == MonitoringErrorKind::NotFound => {}
                Err(version_error) => return Err(version_error),
            }
        }
        Err(error) => return Err(error),
    }

    match probe_endpoint(client, api_url(base_url, 3, "status")?).await {
        Ok(()) => Ok(3),
        Err(error) if error.kind == MonitoringErrorKind::NotFound => Err(MonitoringError::new(
            MonitoringErrorKind::NotFound,
            "Glances REST API v3/v4 is unavailable.",
        )),
        Err(error) => Err(error),
    }
}

async fn probe_endpoint(client: &Client, url: Url) -> Result<(), MonitoringError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(classify_request_error)?;
    classify_status(response.status())
}

async fn fetch_snapshot(
    client: &Client,
    base_url: &Url,
    api_version: u8,
    sampled_at: u64,
) -> ServerMetrics {
    let cpu_url = match api_url(base_url, api_version, "cpu") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };
    let memory_url = match api_url(base_url, api_version, "mem") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };
    let network_url = match api_url(base_url, api_version, "network") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };
    let sensors_url = match api_url(base_url, api_version, "sensors") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };
    let filesystems_url = match api_url(base_url, api_version, "fs") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };
    let uptime_url = match api_url(base_url, api_version, "uptime") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };
    let load_url = match api_url(base_url, api_version, "load") {
        Ok(url) => url,
        Err(error) => return ServerMetrics::unavailable(sampled_at, error.message),
    };

    let (cpu, memory, network, sensors, filesystems, uptime, load) = futures_util::join!(
        fetch_json::<CpuStats>(client, cpu_url),
        fetch_json::<MemoryStats>(client, memory_url),
        fetch_json::<Vec<NetworkStats>>(client, network_url),
        fetch_json::<Vec<SensorStats>>(client, sensors_url),
        fetch_json::<Vec<FilesystemStats>>(client, filesystems_url),
        fetch_json::<UptimeStats>(client, uptime_url),
        fetch_json::<LoadStats>(client, load_url),
    );

    assemble_snapshot(
        sampled_at,
        api_version,
        cpu,
        memory,
        network,
        sensors,
        filesystems,
        uptime,
        load,
    )
}

#[allow(clippy::too_many_arguments)]
fn assemble_snapshot(
    sampled_at: u64,
    api_version: u8,
    cpu: Result<CpuStats, MonitoringError>,
    memory: Result<MemoryStats, MonitoringError>,
    network: Result<Vec<NetworkStats>, MonitoringError>,
    sensors: Result<Vec<SensorStats>, MonitoringError>,
    filesystems: Result<Vec<FilesystemStats>, MonitoringError>,
    uptime: Result<UptimeStats, MonitoringError>,
    load: Result<LoadStats, MonitoringError>,
) -> ServerMetrics {
    let response_success_count = [
        cpu.is_ok(),
        memory.is_ok(),
        network.is_ok(),
        sensors.is_ok(),
        filesystems.is_ok(),
    ]
    .into_iter()
    .filter(|succeeded| *succeeded)
    .count();

    if response_success_count == 0 {
        let errors = [
            cpu.as_ref().err(),
            memory.as_ref().err(),
            network.as_ref().err(),
            sensors.as_ref().err(),
            filesystems.as_ref().err(),
        ];
        let selected_error = errors
            .into_iter()
            .flatten()
            .find(|error| error.kind == MonitoringErrorKind::Authentication)
            .or_else(|| errors.into_iter().flatten().next());
        let message = selected_error
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "Glances metrics are unavailable.".into());
        return ServerMetrics::unavailable(sampled_at, message);
    }

    let cpu_percent = cpu
        .as_ref()
        .ok()
        .and_then(|stats| valid_percent(stats.total));
    let (memory_percent, memory_used_bytes, memory_total_bytes) = memory
        .as_ref()
        .ok()
        .map(normalize_memory)
        .unwrap_or((None, None, None));
    let memory_is_usable =
        memory_total_bytes.is_some() && (memory_percent.is_some() || memory_used_bytes.is_some());

    if cpu_percent.is_none() && !memory_is_usable {
        return ServerMetrics::unavailable(
            sampled_at,
            "Glances returned no usable CPU or memory metrics.",
        );
    }

    let plugin_successes = [
        cpu_percent.is_some(),
        memory_is_usable,
        network.is_ok(),
        sensors.is_ok(),
        filesystems.is_ok(),
    ];
    let failed_plugins = plugin_successes
        .into_iter()
        .filter(|succeeded| !succeeded)
        .count();

    ServerMetrics {
        status: MonitoringStatus::Online,
        sampled_at,
        cpu_percent,
        memory_percent,
        memory_used_bytes,
        memory_total_bytes,
        cpu_temperature_c: sensors
            .as_ref()
            .ok()
            .and_then(|stats| select_cpu_temperature(stats)),
        network_download_bytes_per_second: network
            .as_ref()
            .ok()
            .and_then(|stats| aggregate_network_rate(stats, api_version, true)),
        network_upload_bytes_per_second: network
            .as_ref()
            .ok()
            .and_then(|stats| aggregate_network_rate(stats, api_version, false)),
        uptime_seconds: uptime.as_ref().ok().and_then(normalize_uptime),
        load_average_1m: load
            .as_ref()
            .ok()
            .and_then(|stats| valid_non_negative(stats.min1)),
        load_average_5m: load
            .as_ref()
            .ok()
            .and_then(|stats| valid_non_negative(stats.min5)),
        load_average_15m: load
            .as_ref()
            .ok()
            .and_then(|stats| valid_non_negative(stats.min15)),
        disks: filesystems
            .as_ref()
            .ok()
            .map(|stats| normalize_disks(stats))
            .unwrap_or_default(),
        message: (failed_plugins > 0).then(|| {
            format!(
                "{failed_plugins} Glances {} could not be read.",
                if failed_plugins == 1 {
                    "plugin"
                } else {
                    "plugins"
                }
            )
        }),
    }
}

async fn fetch_json<T: DeserializeOwned>(client: &Client, url: Url) -> Result<T, MonitoringError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(classify_request_error)?;
    classify_status(response.status())?;

    if response
        .content_length()
        .is_some_and(|length| length > MAX_PLUGIN_RESPONSE_BYTES as u64)
    {
        return Err(MonitoringError::new(
            MonitoringErrorKind::TooLarge,
            "A Glances plugin response exceeded the safety limit.",
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify_request_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_PLUGIN_RESPONSE_BYTES {
            return Err(MonitoringError::new(
                MonitoringErrorKind::TooLarge,
                "A Glances plugin response exceeded the safety limit.",
            ));
        }
        body.extend_from_slice(&chunk);
    }

    serde_json::from_slice(&body).map_err(|_| {
        MonitoringError::new(
            MonitoringErrorKind::InvalidData,
            "Glances returned an invalid metrics payload.",
        )
    })
}

fn api_url(base_url: &Url, version: u8, plugin: &str) -> Result<Url, MonitoringError> {
    let mut root = base_url.clone();
    root.set_query(None);
    root.set_fragment(None);
    let mut path = root.path().trim_end_matches('/').to_string();
    path.push('/');
    root.set_path(&path);
    root.join(&format!("api/{version}/{plugin}")).map_err(|_| {
        MonitoringError::new(
            MonitoringErrorKind::InvalidData,
            "The Glances API URL is invalid.",
        )
    })
}

fn classify_status(status: StatusCode) -> Result<(), MonitoringError> {
    if status.is_success() {
        return Ok(());
    }

    let (kind, message) = match status {
        StatusCode::NOT_FOUND => (
            MonitoringErrorKind::NotFound,
            "The Glances API endpoint was not found.".to_string(),
        ),
        StatusCode::METHOD_NOT_ALLOWED => (
            MonitoringErrorKind::MethodNotAllowed,
            "The Glances API rejected the probe method.".to_string(),
        ),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => (
            MonitoringErrorKind::Authentication,
            "Glances authentication is required; secure credential support is planned for Phase 7."
                .to_string(),
        ),
        _ => (
            MonitoringErrorKind::Http,
            format!("Glances returned HTTP {}.", status.as_u16()),
        ),
    };

    Err(MonitoringError::new(kind, message))
}

fn classify_request_error(error: reqwest::Error) -> MonitoringError {
    if error.is_timeout() {
        return MonitoringError::new(
            MonitoringErrorKind::Timeout,
            "Glances did not respond within 2.5 seconds.",
        );
    }

    if error_chain_mentions_tls(&error) {
        return MonitoringError::new(
            MonitoringErrorKind::Tls,
            "Glances TLS certificate validation failed.",
        );
    }

    if error.is_connect() {
        return MonitoringError::new(
            MonitoringErrorKind::Connection,
            "The Glances server could not be reached.",
        );
    }

    MonitoringError::new(
        MonitoringErrorKind::Request,
        "The Glances metrics request failed.",
    )
}

fn build_client(allow_invalid_local_certificate: bool) -> Result<Client, reqwest::Error> {
    let mut builder = Client::builder()
        .timeout(GLANCES_TIMEOUT)
        .redirect(Policy::none())
        .http1_only()
        .user_agent(GLANCES_USER_AGENT)
        .tls_backend_rustls();

    if allow_invalid_local_certificate {
        builder = builder.tls_danger_accept_invalid_certs(true);
    }

    builder.build()
}

fn error_chain_mentions_tls(error: &reqwest::Error) -> bool {
    let mut messages = error.to_string().to_ascii_lowercase();
    let mut source = error.source();

    while let Some(cause) = source {
        messages.push(' ');
        messages.push_str(&cause.to_string().to_ascii_lowercase());
        source = cause.source();
    }

    ["certificate", "unknown issuer", "rustls", "tls handshake"]
        .iter()
        .any(|needle| messages.contains(needle))
}

fn valid_percent(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
}

fn valid_non_negative(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn normalize_uptime(stats: &UptimeStats) -> Option<u64> {
    match stats {
        UptimeStats::Text(value) => parse_uptime_text(value),
        UptimeStats::Seconds { seconds } | UptimeStats::Number(seconds) => Some(*seconds),
    }
}

fn parse_uptime_text(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let (days, clock) = if let Some((day_part, clock)) = value.split_once(", ") {
        let mut parts = day_part.split_whitespace();
        let days = parts.next()?.parse::<u64>().ok()?;
        let unit = parts.next()?;
        if parts.next().is_some() || !matches!((days, unit), (1, "day") | (0 | 2.., "days")) {
            return None;
        }
        (days, clock)
    } else {
        (0, value)
    };

    let mut parts = clock.split(':');
    let hours = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() || hours >= 24 || minutes >= 60 || seconds >= 60 {
        return None;
    }

    days.checked_mul(86_400)?
        .checked_add(hours.checked_mul(3_600)?)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)
}

fn normalize_memory(stats: &MemoryStats) -> (Option<f64>, Option<u64>, Option<u64>) {
    let Some(total_bytes) = stats.total.filter(|total| *total > 0) else {
        return (None, None, None);
    };
    let used_bytes = stats.used.map(|used| used.min(total_bytes)).or_else(|| {
        stats
            .available
            .filter(|available| *available <= total_bytes)
            .map(|available| total_bytes - available)
    });
    let percent = valid_percent(stats.percent)
        .or_else(|| used_bytes.map(|used| (used as f64 / total_bytes as f64) * 100.0));

    (percent, used_bytes, Some(total_bytes))
}

fn aggregate_network_rate(stats: &[NetworkStats], api_version: u8, receive: bool) -> Option<f64> {
    let eligible = |stat: &&NetworkStats| {
        stat.is_up != Some(false)
            && stat
                .interface_name
                .as_deref()
                .is_some_and(|name| !is_loopback_interface(name))
    };
    let preferred = stats
        .iter()
        .filter(eligible)
        .filter(|stat| {
            stat.interface_name
                .as_deref()
                .is_some_and(|name| !is_virtual_interface(name))
        })
        .collect::<Vec<_>>();
    let selected = if preferred.is_empty() {
        stats.iter().filter(eligible).collect::<Vec<_>>()
    } else {
        preferred
    };

    let mut saw_rate = false;
    let total = selected
        .into_iter()
        .filter_map(|stat| {
            let value = if api_version >= 4 {
                if receive {
                    stat.bytes_recv_rate_per_sec
                } else {
                    stat.bytes_sent_rate_per_sec
                }
            } else {
                let bits_per_second = if receive { stat.rx } else { stat.tx };
                bits_per_second.map(|value| value / 8.0)
            };
            let value = valid_non_negative(value)?;
            saw_rate = true;
            Some(value)
        })
        .sum::<f64>();

    saw_rate.then_some(total)
}

fn is_loopback_interface(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "lo" || name.starts_with("lo0") || name.starts_with("loopback")
}

fn is_virtual_interface(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["docker", "veth", "br-", "virbr", "vmnet", "tun", "tap"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn select_cpu_temperature(stats: &[SensorStats]) -> Option<f64> {
    stats
        .iter()
        .filter_map(|sensor| {
            let label = sensor.label.as_deref()?.to_ascii_lowercase();
            let sensor_type = sensor.sensor_type.as_deref().unwrap_or_default();
            let unit = sensor.unit.as_deref().unwrap_or("C");
            let value = sensor.value?;

            if !value.is_finite()
                || !(-50.0..=150.0).contains(&value)
                || !sensor_type.starts_with("temperature")
                || !unit.eq_ignore_ascii_case("c")
            {
                return None;
            }

            let score = if label.contains("package") {
                5
            } else if label.contains("tctl") || label.contains("tdie") {
                4
            } else if label.contains("cpu") {
                3
            } else if label.contains("physical id") {
                2
            } else if label.starts_with("core ") {
                1
            } else {
                return None;
            };

            Some((score, value))
        })
        .max_by(|(left_score, left_value), (right_score, right_value)| {
            left_score
                .cmp(right_score)
                .then_with(|| left_value.total_cmp(right_value))
        })
        .map(|(_, value)| value)
}

fn normalize_disks(stats: &[FilesystemStats]) -> Vec<DiskMetric> {
    let mut selected: HashMap<(String, u64), &FilesystemStats> = HashMap::new();

    for stat in stats {
        let Some(total_bytes) = stat.size.filter(|size| *size > 0) else {
            continue;
        };
        let mount_point = stat.mnt_point.as_deref().unwrap_or_default();

        if mount_point.is_empty() || is_pseudo_mount(mount_point) {
            continue;
        }

        let device_name = stat
            .device_name
            .clone()
            .unwrap_or_else(|| mount_point.to_string());
        let key = (device_name, total_bytes);

        selected
            .entry(key)
            .and_modify(|current| {
                let current_mount = current.mnt_point.as_deref().unwrap_or_default();
                if mount_preference(mount_point) < mount_preference(current_mount) {
                    *current = stat;
                }
            })
            .or_insert(stat);
    }

    let mut disks = selected
        .into_values()
        .filter_map(|stat| {
            let mount_point = stat.mnt_point.clone()?;
            let total_bytes = stat.size?;
            let source_percent = valid_percent(stat.percent);
            let used_bytes = stat.used.map(|used| used.min(total_bytes)).or_else(|| {
                source_percent
                    .map(|percent| ((percent / 100.0) * total_bytes as f64).round() as u64)
            })?;
            let percent =
                source_percent.or_else(|| Some((used_bytes as f64 / total_bytes as f64) * 100.0));

            Some(DiskMetric {
                name: stat
                    .device_name
                    .clone()
                    .unwrap_or_else(|| mount_point.clone()),
                mount_point,
                used_bytes,
                total_bytes,
                percent,
            })
        })
        .collect::<Vec<_>>();
    let root = disks
        .iter()
        .position(|disk| disk.mount_point == "/")
        .map(|index| disks.remove(index));
    disks.sort_by_key(|disk| std::cmp::Reverse(disk.total_bytes));
    disks.truncate(if root.is_some() { 3 } else { 4 });

    if let Some(root) = root {
        disks.insert(0, root);
    }

    disks
}

fn is_pseudo_mount(mount_point: &str) -> bool {
    ["/etc", "/proc", "/sys", "/run", "/dev"]
        .iter()
        .any(|prefix| {
            mount_point == *prefix
                || mount_point
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
}

fn mount_preference(mount_point: &str) -> (u8, usize) {
    let priority = match mount_point {
        "/" => 0,
        "/host" => 1,
        _ => 2,
    };
    (priority, mount_point.len())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_preserves_a_configured_prefix() {
        let base = Url::parse("http://192.168.1.10:61208/glances/").unwrap();
        assert_eq!(
            api_url(&base, 4, "cpu").unwrap().as_str(),
            "http://192.168.1.10:61208/glances/api/4/cpu"
        );
    }

    #[test]
    fn v3_network_rates_are_converted_from_bits_to_bytes() {
        let stats = vec![NetworkStats {
            interface_name: Some("eth0".into()),
            is_up: Some(true),
            bytes_recv_rate_per_sec: None,
            bytes_sent_rate_per_sec: None,
            rx: Some(8_000.0),
            tx: Some(16_000.0),
        }];

        assert_eq!(aggregate_network_rate(&stats, 3, true), Some(1_000.0));
        assert_eq!(aggregate_network_rate(&stats, 3, false), Some(2_000.0));
    }

    #[test]
    fn network_ignores_loopback_and_prefers_physical_interfaces() {
        let payload = r#"[
          {"interface_name":"lo","bytes_recv_rate_per_sec":9000,"bytes_sent_rate_per_sec":9000},
          {"interface_name":"docker0","bytes_recv_rate_per_sec":5000,"bytes_sent_rate_per_sec":5000},
          {"interface_name":"eth0","bytes_recv_rate_per_sec":1200,"bytes_sent_rate_per_sec":300}
        ]"#;
        let stats: Vec<NetworkStats> = serde_json::from_str(payload).unwrap();

        assert_eq!(aggregate_network_rate(&stats, 4, true), Some(1_200.0));
        assert_eq!(aggregate_network_rate(&stats, 4, false), Some(300.0));
    }

    #[test]
    fn package_temperature_wins_over_ambient_and_individual_cores() {
        let payload = r#"[
          {"label":"Ambient","unit":"C","value":70,"type":"temperature_core"},
          {"label":"Core 0","unit":"C","value":52,"type":"temperature_core"},
          {"label":"Package id 0","unit":"C","value":47,"type":"temperature_core"}
        ]"#;
        let stats: Vec<SensorStats> = serde_json::from_str(payload).unwrap();

        assert_eq!(select_cpu_temperature(&stats), Some(47.0));
    }

    #[test]
    fn cpu_named_temperature_and_memory_fallbacks_are_normalized() {
        let sensors: Vec<SensorStats> = serde_json::from_str(
            r#"[{"label":"cpu_thermal","unit":"C","value":44,"type":"temperature_core"}]"#,
        )
        .unwrap();
        let memory = MemoryStats {
            percent: None,
            used: None,
            total: Some(1_000),
            available: Some(250),
        };

        assert_eq!(select_cpu_temperature(&sensors), Some(44.0));
        assert_eq!(
            normalize_memory(&memory),
            (Some(75.0), Some(750), Some(1_000))
        );
        assert_eq!(
            normalize_memory(&MemoryStats {
                percent: None,
                used: None,
                total: None,
                available: None,
            }),
            (None, None, None)
        );
    }

    #[test]
    fn uptime_accepts_v3_v4_text_and_seconds_payloads() {
        let cases = [
            (r#""1:27:01""#, 5_221),
            (r#""7 days, 20:30:06""#, 678_606),
            (r#"{"seconds":99691}"#, 99_691),
            (r#"99691"#, 99_691),
        ];

        for (payload, expected) in cases {
            let stats: UptimeStats = serde_json::from_str(payload).unwrap();
            assert_eq!(normalize_uptime(&stats), Some(expected));
        }
    }

    #[test]
    fn uptime_rejects_malformed_or_overflowing_text() {
        for value in [
            "",
            "1 day, 24:00:00",
            "1 days, 00:00:00",
            "2 days, 00:60:00",
            "18446744073709551615 days, 00:00:00",
        ] {
            assert_eq!(
                parse_uptime_text(value),
                None,
                "unexpectedly parsed {value}"
            );
        }
    }

    #[test]
    fn load_averages_are_independently_sanitized() {
        let stats: LoadStats =
            serde_json::from_str(r#"{"min1":0.75,"min5":1.25,"min15":2.5,"cpucore":8}"#).unwrap();

        assert_eq!(valid_non_negative(stats.min1), Some(0.75));
        assert_eq!(valid_non_negative(stats.min5), Some(1.25));
        assert_eq!(valid_non_negative(stats.min15), Some(2.5));
        assert_eq!(valid_non_negative(Some(-0.1)), None);
        assert_eq!(valid_non_negative(Some(f64::INFINITY)), None);
    }

    #[test]
    fn optional_uptime_and_load_failures_do_not_degrade_core_snapshot() {
        let optional_error = || {
            MonitoringError::new(
                MonitoringErrorKind::NotFound,
                "The optional Glances plugin is unavailable.",
            )
        };
        let snapshot = assemble_snapshot(
            123,
            4,
            Ok(CpuStats { total: Some(20.0) }),
            Ok(MemoryStats {
                percent: Some(40.0),
                used: Some(400),
                total: Some(1_000),
                available: Some(600),
            }),
            Ok(Vec::new()),
            Ok(Vec::new()),
            Ok(Vec::new()),
            Err(optional_error()),
            Err(optional_error()),
        );

        assert_eq!(snapshot.status, MonitoringStatus::Online);
        assert_eq!(snapshot.message, None);
        assert_eq!(snapshot.uptime_seconds, None);
        assert_eq!(snapshot.load_average_1m, None);
        assert_eq!(snapshot.load_average_5m, None);
        assert_eq!(snapshot.load_average_15m, None);
    }

    #[test]
    fn snapshot_serializes_the_phase_6_1_dto_fields() {
        let snapshot = assemble_snapshot(
            123,
            4,
            Ok(CpuStats { total: Some(20.0) }),
            Ok(MemoryStats {
                percent: Some(40.0),
                used: Some(400),
                total: Some(1_000),
                available: Some(600),
            }),
            Ok(Vec::new()),
            Ok(Vec::new()),
            Ok(Vec::new()),
            Ok(UptimeStats::Seconds { seconds: 86_401 }),
            Ok(LoadStats {
                min1: Some(0.5),
                min5: Some(0.75),
                min15: Some(1.0),
            }),
        );
        let value = serde_json::to_value(snapshot).unwrap();

        assert_eq!(value["uptimeSeconds"], 86_401);
        assert_eq!(value["loadAverage1m"], 0.5);
        assert_eq!(value["loadAverage5m"], 0.75);
        assert_eq!(value["loadAverage15m"], 1.0);
    }

    #[test]
    fn disks_deduplicate_bind_mounts_and_ignore_pseudo_mounts() {
        let payload = r#"[
          {"device_name":"/dev/sdb2","mnt_point":"/host","size":1000,"used":400,"percent":40},
          {"device_name":"/dev/sdb2","mnt_point":"/etc/hosts","size":1000,"used":400,"percent":40},
          {"device_name":"/dev/sda1","mnt_point":"/host/srv/ssd","size":500,"used":100,"percent":20}
        ]"#;
        let stats: Vec<FilesystemStats> = serde_json::from_str(payload).unwrap();
        let disks = normalize_disks(&stats);

        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].mount_point, "/host");
        assert_eq!(disks[1].mount_point, "/host/srv/ssd");
    }

    #[test]
    fn disks_keep_root_derive_used_and_filter_exact_dev_mount() {
        let payload = r#"[
          {"device_name":"root","mnt_point":"/","size":10,"percent":50},
          {"device_name":"devtmpfs","mnt_point":"/dev","size":9999,"used":1,"percent":0.01},
          {"device_name":"d1","mnt_point":"/data/1","size":1000,"used":100,"percent":10},
          {"device_name":"d2","mnt_point":"/data/2","size":900,"used":100,"percent":11.1},
          {"device_name":"d3","mnt_point":"/data/3","size":800,"used":100,"percent":12.5},
          {"device_name":"d4","mnt_point":"/data/4","size":700,"used":100,"percent":14.3}
        ]"#;
        let stats: Vec<FilesystemStats> = serde_json::from_str(payload).unwrap();
        let disks = normalize_disks(&stats);

        assert_eq!(disks.len(), 4);
        assert_eq!(disks[0].mount_point, "/");
        assert_eq!(disks[0].used_bytes, 5);
        assert!(disks.iter().all(|disk| disk.mount_point != "/dev"));
    }
}
