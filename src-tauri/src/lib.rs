mod desktop_widgets;
mod health;
mod monitoring;
mod service_webviews;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let service_catalog = service_webviews::ServiceCatalog::from_bundled_config()
        .expect("the bundled service catalog could not be initialized");
    let desktop_widget_broker = desktop_widgets::DesktopWidgetBroker::new(&service_catalog);

    tauri::Builder::default()
        .manage(
            health::HealthClients::new()
                .expect("the health-check HTTP clients could not be initialized"),
        )
        .manage(
            monitoring::GlancesClients::new()
                .expect("the Glances HTTP clients could not be initialized"),
        )
        .manage(service_catalog)
        .manage(desktop_widget_broker)
        .manage(service_webviews::ServiceWebviewRegistry::default())
        .invoke_handler(tauri::generate_handler![
            health::check_service_health,
            monitoring::get_server_metrics,
            desktop_widgets::get_desktop_widget_snapshot,
            desktop_widgets::set_desktop_widget_visibility,
            service_webviews::open_service_webview,
            service_webviews::activate_service_webview,
            service_webviews::hide_service_webviews,
            service_webviews::reconcile_service_webviews,
            service_webviews::park_service_webview,
            service_webviews::close_service_webview,
            service_webviews::open_service_in_system_browser,
        ])
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
