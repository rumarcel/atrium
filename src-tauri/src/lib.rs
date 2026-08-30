mod health;
mod monitoring;
mod service_webviews;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(
            health::HealthClients::new()
                .expect("the health-check HTTP clients could not be initialized"),
        )
        .manage(
            monitoring::GlancesClients::new()
                .expect("the Glances HTTP clients could not be initialized"),
        )
        .manage(
            service_webviews::ServiceCatalog::from_bundled_config()
                .expect("the bundled service catalog could not be initialized"),
        )
        .manage(service_webviews::ServiceWebviewRegistry::default())
        .invoke_handler(tauri::generate_handler![
            health::check_service_health,
            monitoring::get_server_metrics,
            service_webviews::open_service_webview,
            service_webviews::activate_service_webview,
            service_webviews::hide_service_webviews,
            service_webviews::reconcile_service_webviews,
            service_webviews::park_service_webview,
            service_webviews::close_service_webview,
            service_webviews::open_service_in_system_browser,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
