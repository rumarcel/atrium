use crate::{
    health::{self, HealthCheckResult, HealthClients, HealthReason, HealthStatus},
    monitoring::{self, DiskMetric, GlancesClients, MonitoringStatus, ServerMetrics},
    service_webviews::ServiceCatalog,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

const METRICS_CADENCE: Duration = Duration::from_secs(5);
const HEALTH_CADENCE: Duration = Duration::from_secs(45);
const SCHEDULER_TICK: Duration = Duration::from_millis(200);
const MAX_TREND_SAMPLES: usize = 24;
const MAX_CONCURRENT_HEALTH_CHECKS: usize = 8;
const SERVER_NAME: &str = "Personal Hub Server";
const FALLBACK_SERVER_ADDRESS: &str = "192.168.1.10";

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopWidgetKind {
    Server,
    Storage,
    Services,
}

impl DesktopWidgetKind {
    fn window_label(self) -> &'static str {
        match self {
            Self::Server => "widget-server",
            Self::Storage => "widget-storage",
            Self::Services => "widget-services",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWidgetMetrics {
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
}

impl From<&ServerMetrics> for DesktopWidgetMetrics {
    fn from(metrics: &ServerMetrics) -> Self {
        Self {
            cpu_percent: metrics.cpu_percent,
            memory_percent: metrics.memory_percent,
            memory_used_bytes: metrics.memory_used_bytes,
            memory_total_bytes: metrics.memory_total_bytes,
            cpu_temperature_c: metrics.cpu_temperature_c,
            network_download_bytes_per_second: metrics.network_download_bytes_per_second,
            network_upload_bytes_per_second: metrics.network_upload_bytes_per_second,
            uptime_seconds: metrics.uptime_seconds,
            load_average_1m: metrics.load_average_1m,
            load_average_5m: metrics.load_average_5m,
            load_average_15m: metrics.load_average_15m,
            disks: metrics.disks.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWidgetService {
    id: String,
    name: String,
    status: HealthStatus,
    message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWidgetTrend {
    sampled_at: u64,
    cpu_percent: Option<f64>,
    memory_percent: Option<f64>,
    network_download_bytes_per_second: Option<f64>,
    network_upload_bytes_per_second: Option<f64>,
    load_average_1m: Option<f64>,
}

impl From<&ServerMetrics> for DesktopWidgetTrend {
    fn from(metrics: &ServerMetrics) -> Self {
        Self {
            sampled_at: metrics.sampled_at,
            cpu_percent: metrics.cpu_percent,
            memory_percent: metrics.memory_percent,
            network_download_bytes_per_second: metrics.network_download_bytes_per_second,
            network_upload_bytes_per_second: metrics.network_upload_bytes_per_second,
            load_average_1m: metrics.load_average_1m,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWidgetSnapshot {
    status: MonitoringStatus,
    sampled_at: u64,
    server_name: String,
    server_address: String,
    metrics: DesktopWidgetMetrics,
    services: Vec<DesktopWidgetService>,
    trends: Vec<DesktopWidgetTrend>,
    message: Option<String>,
}

struct BrokerInner {
    visible_widgets: HashSet<DesktopWidgetKind>,
    scheduler_running: bool,
    metrics_in_flight: bool,
    health_in_flight: bool,
    next_metrics_due: Instant,
    next_health_due: Instant,
    snapshot: DesktopWidgetSnapshot,
    trends: VecDeque<DesktopWidgetTrend>,
    health_sampled_at: Option<u64>,
}

pub struct DesktopWidgetBroker {
    inner: Mutex<BrokerInner>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PollClaims {
    metrics: bool,
    health: bool,
}

impl DesktopWidgetBroker {
    pub fn new(catalog: &ServiceCatalog) -> Self {
        let sampled_at = unix_time_ms();
        let server_address = catalog
            .resolve_endpoint("glances")
            .ok()
            .and_then(|endpoint| endpoint.url.host_str().map(str::to_owned))
            .unwrap_or_else(|| FALLBACK_SERVER_ADDRESS.to_owned());
        let now = Instant::now();

        Self {
            inner: Mutex::new(BrokerInner {
                visible_widgets: HashSet::new(),
                scheduler_running: false,
                metrics_in_flight: false,
                health_in_flight: false,
                next_metrics_due: now,
                next_health_due: now,
                snapshot: DesktopWidgetSnapshot {
                    status: MonitoringStatus::Unavailable,
                    sampled_at,
                    server_name: SERVER_NAME.to_owned(),
                    server_address,
                    metrics: DesktopWidgetMetrics::default(),
                    services: Vec::new(),
                    trends: Vec::new(),
                    message: Some("Waiting for a visible desktop card.".into()),
                },
                trends: VecDeque::with_capacity(MAX_TREND_SAMPLES),
                health_sampled_at: None,
            }),
        }
    }

    fn set_visibility(&self, kind: DesktopWidgetKind, visible: bool) -> Result<bool, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "The desktop widget broker is unavailable.".to_string())?;
        let was_empty = inner.visible_widgets.is_empty();

        if visible {
            inner.visible_widgets.insert(kind);
        } else {
            inner.visible_widgets.remove(&kind);
        }

        if was_empty && !inner.visible_widgets.is_empty() {
            let now = Instant::now();
            inner.next_metrics_due = now;
            inner.next_health_due = now;
        }

        if !inner.visible_widgets.is_empty() && !inner.scheduler_running {
            inner.scheduler_running = true;
            return Ok(true);
        }

        Ok(false)
    }

    fn snapshot(&self, kind: DesktopWidgetKind) -> Result<DesktopWidgetSnapshot, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "The desktop widget broker is unavailable.".to_string())?;
        let mut snapshot = inner.snapshot.clone();

        match kind {
            DesktopWidgetKind::Server => {
                snapshot.metrics.disks.clear();
                snapshot.services.clear();
            }
            DesktopWidgetKind::Storage => {
                let disks = std::mem::take(&mut snapshot.metrics.disks);
                snapshot.metrics = DesktopWidgetMetrics {
                    disks,
                    ..DesktopWidgetMetrics::default()
                };
                snapshot.services.clear();
                snapshot.trends.clear();
            }
            DesktopWidgetKind::Services => {
                snapshot.status = if inner.health_sampled_at.is_some() {
                    MonitoringStatus::Online
                } else {
                    MonitoringStatus::Unavailable
                };
                snapshot.sampled_at = inner.health_sampled_at.unwrap_or(snapshot.sampled_at);
                snapshot.metrics = DesktopWidgetMetrics::default();
                snapshot.trends.clear();
                snapshot.message = inner
                    .health_sampled_at
                    .is_none()
                    .then(|| "Waiting for server service health checks.".into());
            }
        }

        Ok(snapshot)
    }

    fn claim_due_polls(&self, now: Instant) -> Result<Option<PollClaims>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "The desktop widget broker is unavailable.".to_string())?;

        if inner.visible_widgets.is_empty() {
            inner.scheduler_running = false;
            return Ok(None);
        }

        let needs_metrics = inner
            .visible_widgets
            .iter()
            .any(|kind| matches!(kind, DesktopWidgetKind::Server | DesktopWidgetKind::Storage));
        let needs_health = inner.visible_widgets.contains(&DesktopWidgetKind::Services);

        let mut claims = PollClaims::default();
        if needs_metrics && !inner.metrics_in_flight && now >= inner.next_metrics_due {
            inner.metrics_in_flight = true;
            inner.next_metrics_due = now + METRICS_CADENCE;
            claims.metrics = true;
        }
        if needs_health && !inner.health_in_flight && now >= inner.next_health_due {
            inner.health_in_flight = true;
            inner.next_health_due = now + HEALTH_CADENCE;
            claims.health = true;
        }

        Ok(Some(claims))
    }

    fn update_metrics(&self, metrics: ServerMetrics) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };

        let trend = DesktopWidgetTrend::from(&metrics);
        if inner.trends.len() == MAX_TREND_SAMPLES {
            inner.trends.pop_front();
        }
        inner.trends.push_back(trend);

        inner.snapshot.status = metrics.status;
        inner.snapshot.sampled_at = metrics.sampled_at;
        inner.snapshot.metrics = DesktopWidgetMetrics::from(&metrics);
        inner.snapshot.trends = inner.trends.iter().cloned().collect();
        inner.snapshot.message = metrics.message;
        inner.metrics_in_flight = false;
    }

    fn update_services(&self, services: Vec<DesktopWidgetService>, sampled_at: u64) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };

        inner.snapshot.services = services;
        inner.health_sampled_at = Some(sampled_at);
        inner.health_in_flight = false;
    }
}

#[tauri::command]
pub fn get_desktop_widget_snapshot(
    caller: tauri::Webview,
    kind: DesktopWidgetKind,
    broker: tauri::State<'_, DesktopWidgetBroker>,
) -> Result<DesktopWidgetSnapshot, String> {
    authorize_widget(caller.label(), kind)?;
    broker.snapshot(kind)
}

#[tauri::command]
pub fn set_desktop_widget_visibility(
    caller: tauri::Webview,
    kind: DesktopWidgetKind,
    visible: bool,
    app: AppHandle,
    broker: tauri::State<'_, DesktopWidgetBroker>,
) -> Result<(), String> {
    authorize_widget(caller.label(), kind)?;
    if broker.set_visibility(kind, visible)? {
        start_scheduler(app);
    }
    Ok(())
}

fn authorize_widget(caller_label: &str, kind: DesktopWidgetKind) -> Result<(), String> {
    if caller_label == kind.window_label() {
        Ok(())
    } else {
        Err("Desktop widget data is available only to its matching trusted card window.".into())
    }
}

fn start_scheduler(app: AppHandle) {
    std::thread::Builder::new()
        .name("desktop-widget-broker".into())
        .spawn(move || loop {
            let broker = app.state::<DesktopWidgetBroker>();
            let claims = match broker.claim_due_polls(Instant::now()) {
                Ok(Some(claims)) => claims,
                Ok(None) | Err(_) => return,
            };

            if claims.metrics {
                spawn_metrics_poll(app.clone());
            }
            if claims.health {
                spawn_health_poll(app.clone());
            }

            std::thread::sleep(SCHEDULER_TICK);
        })
        .expect("the desktop widget broker scheduler could not be started");
}

fn spawn_metrics_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let metrics = {
            let catalog = app.state::<ServiceCatalog>();
            let clients = app.state::<GlancesClients>();
            monitoring::collect_server_metrics(&catalog, &clients).await
        };
        app.state::<DesktopWidgetBroker>().update_metrics(metrics);
    });
}

fn spawn_health_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let targets = app.state::<ServiceCatalog>().enabled_health_targets();
        let clients = app.state::<HealthClients>();
        let mut services = stream::iter(targets.into_iter().map(|target| {
            let name = target.name.clone();
            let clients = &*clients;
            async move {
                let result = health::check_trusted_service_health(target, clients).await;
                desktop_service_from_health(name, result)
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_HEALTH_CHECKS)
        .collect::<Vec<_>>()
        .await;
        services.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        app.state::<DesktopWidgetBroker>()
            .update_services(services, unix_time_ms());
    });
}

fn desktop_service_from_health(name: String, result: HealthCheckResult) -> DesktopWidgetService {
    let uses_expected_local_tls_exception = result.reason == Some(HealthReason::TlsException);

    DesktopWidgetService {
        id: result.service_id,
        name,
        status: if uses_expected_local_tls_exception {
            HealthStatus::Online
        } else {
            result.status
        },
        message: if uses_expected_local_tls_exception {
            None
        } else {
            result.message
        },
    }
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

    fn broker() -> DesktopWidgetBroker {
        let catalog = ServiceCatalog::from_bundled_config().unwrap();
        DesktopWidgetBroker::new(&catalog)
    }

    fn sample(sampled_at: u64) -> ServerMetrics {
        ServerMetrics {
            status: MonitoringStatus::Online,
            sampled_at,
            cpu_percent: Some(25.0),
            memory_percent: Some(50.0),
            memory_used_bytes: Some(500),
            memory_total_bytes: Some(1_000),
            cpu_temperature_c: Some(42.0),
            network_download_bytes_per_second: Some(2_000.0),
            network_upload_bytes_per_second: Some(1_000.0),
            uptime_seconds: Some(86_400),
            load_average_1m: Some(0.5),
            load_average_5m: Some(0.4),
            load_average_15m: Some(0.3),
            disks: Vec::new(),
            message: None,
        }
    }

    fn health_result(
        status: HealthStatus,
        reason: Option<HealthReason>,
        message: Option<&str>,
    ) -> HealthCheckResult {
        HealthCheckResult {
            service_id: "cockpit".into(),
            status,
            status_code: Some(200),
            latency_ms: 1,
            checked_at_unix_ms: 1,
            reason,
            message: message.map(str::to_owned),
            tls_exception_used: reason == Some(HealthReason::TlsException),
        }
    }

    #[test]
    fn labels_are_exactly_bound_to_their_widget_kind() {
        assert!(authorize_widget("widget-server", DesktopWidgetKind::Server).is_ok());
        assert!(authorize_widget("widget-storage", DesktopWidgetKind::Storage).is_ok());
        assert!(authorize_widget("widget-services", DesktopWidgetKind::Services).is_ok());
        assert!(authorize_widget("main", DesktopWidgetKind::Server).is_err());
        assert!(authorize_widget("widget-server", DesktopWidgetKind::Storage).is_err());
        assert!(authorize_widget("widget-server-extra", DesktopWidgetKind::Server).is_err());
    }

    #[test]
    fn polling_starts_only_while_at_least_one_widget_is_visible() {
        let broker = broker();
        assert_eq!(broker.claim_due_polls(Instant::now()).unwrap(), None);

        assert!(broker
            .set_visibility(DesktopWidgetKind::Server, true)
            .unwrap());
        let claims = broker
            .claim_due_polls(Instant::now() + Duration::from_millis(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            claims,
            PollClaims {
                metrics: true,
                health: false
            }
        );

        broker
            .set_visibility(DesktopWidgetKind::Server, false)
            .unwrap();
        assert_eq!(broker.claim_due_polls(Instant::now()).unwrap(), None);
    }

    #[test]
    fn multiple_metric_widgets_share_one_poll_claim() {
        let broker = broker();
        assert!(broker
            .set_visibility(DesktopWidgetKind::Server, true)
            .unwrap());
        assert!(!broker
            .set_visibility(DesktopWidgetKind::Storage, true)
            .unwrap());

        let first = broker
            .claim_due_polls(Instant::now() + Duration::from_millis(1))
            .unwrap()
            .unwrap();
        let duplicate = broker
            .claim_due_polls(Instant::now() + Duration::from_millis(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            first,
            PollClaims {
                metrics: true,
                health: false
            }
        );
        assert_eq!(duplicate, PollClaims::default());
    }

    #[test]
    fn the_services_widget_claims_only_the_health_poll() {
        let broker = broker();
        assert!(broker
            .set_visibility(DesktopWidgetKind::Services, true)
            .unwrap());

        let claims = broker
            .claim_due_polls(Instant::now() + Duration::from_millis(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            claims,
            PollClaims {
                metrics: false,
                health: true
            }
        );
    }

    #[test]
    fn expected_local_tls_exceptions_do_not_create_attention_alerts() {
        let service = desktop_service_from_health(
            "Cockpit".into(),
            health_result(
                HealthStatus::Warning,
                Some(HealthReason::TlsException),
                Some("Expected local certificate exception."),
            ),
        );

        assert_eq!(service.status, HealthStatus::Online);
        assert_eq!(service.message, None);
    }

    #[test]
    fn real_health_failures_remain_visible_to_the_attention_card() {
        let service = desktop_service_from_health(
            "Cockpit".into(),
            health_result(
                HealthStatus::Offline,
                Some(HealthReason::Connection),
                Some("The service could not be reached."),
            ),
        );

        assert_eq!(service.status, HealthStatus::Offline);
        assert_eq!(
            service.message.as_deref(),
            Some("The service could not be reached.")
        );
    }

    #[test]
    fn metric_history_is_bounded_to_the_latest_24_samples() {
        let broker = broker();
        for sampled_at in 0..30 {
            broker.update_metrics(sample(sampled_at));
        }

        let snapshot = broker.snapshot(DesktopWidgetKind::Server).unwrap();
        assert_eq!(snapshot.trends.len(), MAX_TREND_SAMPLES);
        assert_eq!(snapshot.trends.first().unwrap().sampled_at, 6);
        assert_eq!(snapshot.trends.last().unwrap().sampled_at, 29);
    }

    #[test]
    fn snapshot_matches_the_frontend_camel_case_contract() {
        let broker = broker();
        broker.update_metrics(sample(123));
        let value =
            serde_json::to_value(broker.snapshot(DesktopWidgetKind::Server).unwrap()).unwrap();

        assert_eq!(value["status"], "online");
        assert_eq!(value["sampledAt"], 123);
        assert_eq!(value["serverName"], SERVER_NAME);
        assert_eq!(value["serverAddress"], FALLBACK_SERVER_ADDRESS);
        assert_eq!(value["metrics"]["cpuPercent"], 25.0);
        assert_eq!(value["trends"][0]["loadAverage1m"], 0.5);
        assert!(value.get("services").unwrap().is_array());
    }

    #[test]
    fn snapshots_are_scoped_to_the_authorized_card() {
        let broker = broker();
        broker.update_metrics(sample(123));
        broker.update_services(
            vec![DesktopWidgetService {
                id: "jellyfin".into(),
                name: "Jellyfin".into(),
                status: HealthStatus::Online,
                message: None,
            }],
            456,
        );

        let server = broker.snapshot(DesktopWidgetKind::Server).unwrap();
        assert!(server.services.is_empty());
        assert!(server.metrics.disks.is_empty());
        assert_eq!(server.trends.len(), 1);

        let storage = broker.snapshot(DesktopWidgetKind::Storage).unwrap();
        assert!(storage.services.is_empty());
        assert!(storage.trends.is_empty());

        let services = broker.snapshot(DesktopWidgetKind::Services).unwrap();
        assert_eq!(services.status, MonitoringStatus::Online);
        assert_eq!(services.sampled_at, 456);
        assert_eq!(services.services.len(), 1);
        assert!(services.metrics.disks.is_empty());
        assert!(services.trends.is_empty());
    }
}
