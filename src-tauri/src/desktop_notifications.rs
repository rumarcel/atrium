//! Opt-in native observations. Remote WebViews cannot submit notification text.
#[cfg(any(windows, test))]
#[derive(Default)]
struct OutageState {
    failures: u8,
    alerted: bool,
}

#[cfg(any(windows, test))]
impl OutageState {
    fn observe(&mut self, unavailable: bool) -> bool {
        if !unavailable {
            *self = Self::default();
            return false;
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 2 && !self.alerted {
            self.alerted = true;
            return true;
        }
        false
    }
}

#[cfg(any(windows, test))]
#[derive(Default)]
struct StorageState {
    sampled: bool,
    alerted: bool,
}

#[cfg(any(windows, test))]
impl StorageState {
    fn observe(&mut self, percent: f64) -> bool {
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return false;
        }
        if !self.sampled {
            self.sampled = true;
            return false;
        }
        if percent <= 85.0 {
            self.alerted = false;
        }
        if percent >= 90.0 && !self.alerted {
            self.alerted = true;
            return true;
        }
        false
    }
}

pub fn setup(app: &tauri::AppHandle) -> Result<(), std::io::Error> {
    #[cfg(windows)]
    return native::setup(app);
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(())
    }
}

#[cfg(windows)]
mod native {
    use super::{OutageState, StorageState};
    use crate::{
        appearance_settings::notification_language_is_turkish,
        desktop_integration::{DesktopIntegrationSettings, DesktopPreferences},
        download_center::{self, DownloadCenterClients},
        health::{self, HealthClients, HealthStatus},
        monitoring::{self, GlancesClients, MonitoringStatus},
        provider_auth::ProviderAuthManager,
        service_webviews::{ServiceCatalog, TrustedAuthenticationTarget},
    };
    use futures_util::{lock::Mutex, stream, StreamExt};
    use std::{
        collections::{HashMap, HashSet},
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    use tauri::{AppHandle, Manager};
    use tauri_plugin_notification::NotificationExt;

    const POLL_INTERVAL: Duration = Duration::from_secs(30);

    #[derive(Default)]
    struct EpochState<T> {
        generation: u64,
        data: T,
    }

    #[derive(Default)]
    struct DiskObservations {
        provider: Option<String>,
        disks: HashMap<String, StorageState>,
    }

    #[derive(Default)]
    struct DownloadObservations {
        provider: Option<String>,
        hashes: Vec<String>,
    }

    #[derive(Clone, Copy)]
    enum Kind {
        Outage,
        Storage,
        Download,
    }

    impl Kind {
        fn enabled(self, preferences: &DesktopPreferences) -> bool {
            match self {
                Self::Outage => preferences.notify_service_outages,
                Self::Storage => preferences.notify_storage_pressure,
                Self::Download => preferences.notify_download_completion,
            }
        }
    }

    fn preferences(app: &AppHandle) -> DesktopPreferences {
        app.state::<DesktopIntegrationSettings>()
            .preferences()
            .unwrap_or_default()
    }

    fn still_enabled(app: &AppHandle, kind: Kind, epoch: &AtomicU64, generation: u64) -> bool {
        epoch.load(Ordering::Acquire) == generation && kind.enabled(&preferences(app))
    }

    fn send(app: &AppHandle, kind: Kind, epoch: &AtomicU64, generation: u64) {
        if !still_enabled(app, kind, epoch, generation) {
            return;
        }
        let (title, body) = match (notification_language_is_turkish(app), kind) {
            (true, Kind::Outage) => ("Servise erişilemiyor", "Bir veya daha fazla sunucu servisine art arda erişilemedi. Ayrıntılar için Personal Hub’ı açın."),
            (false, Kind::Outage) => ("Service unavailable", "One or more server services could not be reached on consecutive checks. Open Personal Hub for details."),
            (true, Kind::Storage) => ("Sunucu diski dolmak üzere", "Bir sunucu diskinin kullanımı %90 veya üzerine çıktı. Ayrıntılar için Personal Hub’ı açın."),
            (false, Kind::Storage) => ("Server storage is nearly full", "A server volume is at least 90% full. Open Personal Hub for details."),
            (true, Kind::Download) => ("İndirme tamamlandı", "İzlenen bir veya daha fazla indirme tamamlandı. Ayrıntılar için Personal Hub’ı açın."),
            (false, Kind::Download) => ("Download completed", "One or more monitored downloads have finished. Open Personal Hub for details."),
        };
        // Static text only: no service addresses, filenames, paths or secrets.
        let _ = app.notification().builder().title(title).body(body).show();
    }

    pub(super) fn setup(app: &AppHandle) -> Result<(), std::io::Error> {
        let app = app.clone();
        std::thread::Builder::new()
            .name("desktop-notifications".into())
            .spawn(move || {
                let health = Arc::new(Mutex::new(
                    EpochState::<HashMap<String, OutageState>>::default(),
                ));
                let disks = Arc::new(Mutex::new(EpochState::<DiskObservations>::default()));
                let downloads = Arc::new(Mutex::new(EpochState::<DownloadObservations>::default()));
                let epoch = Arc::new(AtomicU64::new(1));
                let mut last_preferences = DesktopPreferences::default();
                let mut due = Instant::now();
                loop {
                    let current = preferences(&app);
                    if current != last_preferences {
                        epoch.fetch_add(1, Ordering::AcqRel);
                        last_preferences = current;
                        due = Instant::now();
                    }
                    if Instant::now() >= due {
                        due = Instant::now() + POLL_INTERVAL;
                        let generation = epoch.load(Ordering::Acquire);
                        {
                            let (app, epoch, state) = (app.clone(), epoch.clone(), health.clone());
                            tauri::async_runtime::spawn(async move {
                                if let Some(mut state) = state.try_lock() {
                                    reset_epoch(&mut state, generation);
                                    poll_health(&app, &mut state.data, &epoch, generation).await;
                                };
                            });
                        }
                        {
                            let (app, epoch, state) = (app.clone(), epoch.clone(), disks.clone());
                            tauri::async_runtime::spawn(async move {
                                if let Some(mut state) = state.try_lock() {
                                    reset_epoch(&mut state, generation);
                                    poll_storage(&app, &mut state.data, &epoch, generation).await;
                                };
                            });
                        }
                        {
                            let (app, epoch, state) =
                                (app.clone(), epoch.clone(), downloads.clone());
                            tauri::async_runtime::spawn(async move {
                                if let Some(mut state) = state.try_lock() {
                                    reset_epoch(&mut state, generation);
                                    poll_downloads(&app, &mut state.data, &epoch, generation).await;
                                };
                            });
                        }
                    }
                    // This thread only schedules; slow providers cannot hold up
                    // other notification types or overlap their own next poll.
                    std::thread::sleep(Duration::from_secs(2));
                }
            })?;
        Ok(())
    }

    fn reset_epoch<T: Default>(state: &mut EpochState<T>, generation: u64) {
        if state.generation != generation {
            state.generation = generation;
            state.data = T::default();
        }
    }

    fn health_key(catalog: &ServiceCatalog, id: &str) -> Option<String> {
        let target = catalog.resolve_authentication_target(id).ok()?;
        target.enabled.then(|| health_target_key(&target))
    }

    fn health_target_key(target: &TrustedAuthenticationTarget) -> String {
        format!(
            "{}:{}:{}:{}",
            target.service_id,
            target.catalog_revision,
            target.url,
            target.allow_invalid_local_certificate
        )
    }

    async fn poll_health(
        app: &AppHandle,
        observations: &mut HashMap<String, OutageState>,
        epoch: &AtomicU64,
        generation: u64,
    ) {
        if !still_enabled(app, Kind::Outage, epoch, generation) {
            observations.clear();
            return;
        }
        let catalog = app.state::<ServiceCatalog>();
        let clients = app.state::<HealthClients>();
        let targets = catalog.enabled_health_targets();
        let keys = targets
            .iter()
            .filter_map(|target| health_key(&catalog, &target.id))
            .collect::<HashSet<_>>();
        observations.retain(|key, _| keys.contains(key));
        let mut results = stream::iter(targets.into_iter().filter_map(|target| {
            let latest = catalog.resolve_authentication_target(&target.id).ok()?;
            if !latest.enabled
                || latest.url != target.endpoint.url
                || latest.allow_invalid_local_certificate
                    != target.endpoint.allow_invalid_local_certificate
            {
                return None;
            }
            let key = health_target_key(&latest);
            let clients = &*clients;
            Some(async move {
                (
                    key,
                    health::check_trusted_service_health(target, clients).await,
                )
            })
        }))
        .buffer_unordered(8);
        let mut alerted_targets = Vec::new();
        while let Some((key, result)) = results.next().await {
            if !still_enabled(app, Kind::Outage, epoch, generation) {
                observations.clear();
                return;
            }
            if health_key(&catalog, &result.service_id).as_deref() != Some(key.as_str()) {
                continue;
            }
            // An authentication challenge proves the endpoint is reachable.
            let unavailable = result.status == HealthStatus::Offline
                && !matches!(result.status_code, Some(401 | 403));
            if observations
                .entry(key.clone())
                .or_default()
                .observe(unavailable)
            {
                alerted_targets.push((result.service_id, key));
            }
        }
        if alerted_targets
            .iter()
            .any(|(id, key)| health_key(&catalog, id).as_deref() == Some(key.as_str()))
        {
            send(app, Kind::Outage, epoch, generation);
        }
    }

    fn metrics_provider_key(
        catalog: &ServiceCatalog,
        authentication: &ProviderAuthManager,
    ) -> Option<String> {
        let target = catalog.resolve_authentication_target("glances").ok()?;
        target
            .enabled
            .then(|| authentication.revision_for_target(&target))
    }

    async fn poll_storage(
        app: &AppHandle,
        observations: &mut DiskObservations,
        epoch: &AtomicU64,
        generation: u64,
    ) {
        if !still_enabled(app, Kind::Storage, epoch, generation) {
            *observations = DiskObservations::default();
            return;
        }
        let catalog = app.state::<ServiceCatalog>();
        let authentication = app.state::<ProviderAuthManager>();
        let provider = metrics_provider_key(&catalog, &authentication);
        if observations.provider != provider {
            observations.disks.clear();
            observations.provider = provider.clone();
        }
        if provider.is_none() {
            return;
        }
        let clients = app.state::<GlancesClients>();
        let metrics = monitoring::collect_server_metrics(&catalog, &clients).await;
        if !still_enabled(app, Kind::Storage, epoch, generation)
            || metrics_provider_key(&catalog, &authentication) != provider
        {
            *observations = DiskObservations::default();
            return;
        }
        if metrics.status != MonitoringStatus::Online {
            return;
        }
        let mut current = HashSet::new();
        let mut alert = false;
        for disk in metrics.disks {
            let key = format!("{}\n{}", disk.name, disk.mount_point);
            current.insert(key.clone());
            if let Some(percent) = disk.percent {
                alert |= observations.disks.entry(key).or_default().observe(percent);
            }
        }
        observations.disks.retain(|key, _| current.contains(key));
        if alert {
            send(app, Kind::Storage, epoch, generation);
        }
    }

    async fn poll_downloads(
        app: &AppHandle,
        observations: &mut DownloadObservations,
        epoch: &AtomicU64,
        generation: u64,
    ) {
        if !still_enabled(app, Kind::Download, epoch, generation) {
            *observations = DownloadObservations::default();
            return;
        }
        let catalog = app.state::<ServiceCatalog>();
        let authentication = app.state::<ProviderAuthManager>();
        let provider = download_center::notification_provider_key(&catalog, &authentication);
        if observations.provider != provider {
            observations.hashes.clear();
            observations.provider = provider.clone();
        }
        if provider.is_none() {
            return;
        }
        let clients = app.state::<DownloadCenterClients>();
        let sample = download_center::collect_download_notification_sample(
            &catalog,
            &authentication,
            &clients,
            observations.provider.as_deref(),
            &observations.hashes,
        )
        .await;
        if !still_enabled(app, Kind::Download, epoch, generation)
            || download_center::notification_provider_key(&catalog, &authentication) != provider
        {
            *observations = DownloadObservations::default();
            return;
        }
        if let Ok(Some(sample)) = sample {
            observations.provider = Some(sample.provider_key);
            observations.hashes = sample.incomplete_hashes;
            if sample.confirmed_completed > 0 {
                send(app, Kind::Download, epoch, generation);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outage_requires_two_observations_and_rearms_after_recovery() {
        let mut state = OutageState::default();
        assert!(!state.observe(true));
        assert!(state.observe(true));
        assert!(!state.observe(true));
        assert!(!state.observe(false));
        assert!(!state.observe(true));
        assert!(state.observe(true));
    }

    #[test]
    fn storage_baseline_is_silent_and_rearms_below_threshold() {
        let mut state = StorageState::default();
        assert!(!state.observe(95.0));
        assert!(state.observe(95.0));
        assert!(!state.observe(99.0));
        assert!(!state.observe(87.0));
        assert!(!state.observe(92.0));
        assert!(!state.observe(85.0));
        assert!(state.observe(90.0));
        assert!(!StorageState::default().observe(f64::NAN));
    }
}
