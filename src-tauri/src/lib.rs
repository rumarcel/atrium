mod credential_vault;
mod desktop_widgets;
mod health;
mod monitoring;
mod service_settings;
mod service_webviews;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    let configuration_directory = dirs::config_dir()
        .expect("the application configuration directory is unavailable")
        .join(&context.config().identifier);
    let settings = service_settings::ServiceSettings::initialize(
        configuration_directory,
        service_settings::BUNDLED_SERVICE_CONFIG,
    )
    .expect("the service settings could not be initialized");
    let configuration = settings
        .configuration()
        .expect("the service configuration state is unavailable");
    let catalog = service_webviews::ServiceCatalog::from_configuration(&configuration)
        .expect("the service catalog could not be initialized");
    let desktop_widget_broker = desktop_widgets::DesktopWidgetBroker::new(&catalog);

    tauri::Builder::default()
        .manage(
            health::HealthClients::new()
                .expect("the health-check HTTP clients could not be initialized"),
        )
        .manage(
            monitoring::GlancesClients::new()
                .expect("the Glances HTTP clients could not be initialized"),
        )
        .manage(service_webviews::ServiceWebviewRegistry::default())
        .manage(settings)
        .manage(catalog)
        .manage(desktop_widget_broker)
        .invoke_handler(tauri::generate_handler![
            health::check_service_health,
            monitoring::get_server_metrics,
            desktop_widgets::get_desktop_widget_snapshot,
            desktop_widgets::set_desktop_widget_visibility,
            service_settings::get_service_configuration,
            service_settings::save_service_configuration,
            service_settings::reset_service_configuration,
            service_settings::restore_service_configuration_backup,
            credential_vault::get_service_credential_statuses,
            credential_vault::set_service_credential,
            credential_vault::delete_service_credential,
            service_webviews::open_service_webview,
            service_webviews::activate_service_webview,
            service_webviews::hide_service_webviews,
            service_webviews::reconcile_service_webviews,
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
        .run(context)
        .expect("error while running tauri application");
}
