mod appearance_settings;
mod background_runtime;
mod credential_vault;
mod desktop_integration;
mod desktop_notifications;
mod desktop_widgets;
mod download_center;
mod health;
mod monitoring;
mod provider_auth;
mod server_control;
mod server_inventory;
mod service_discovery;
mod service_settings;
mod service_webviews;

use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    let configuration_directory = dirs::config_dir()
        .expect("the application configuration directory is unavailable")
        .join(&context.config().identifier);
    let settings = service_settings::ServiceSettings::initialize(
        &configuration_directory,
        service_settings::BUNDLED_SERVICE_CONFIG,
    )
    .expect("the service settings could not be initialized");
    let configuration = settings
        .configuration()
        .expect("the service configuration state is unavailable");
    let catalog = service_webviews::ServiceCatalog::from_configuration(&configuration)
        .expect("the service catalog could not be initialized");
    let desktop_widget_broker = desktop_widgets::DesktopWidgetBroker::new(&catalog);
    let provider_auth = provider_auth::ProviderAuthManager::new()
        .expect("the provider-authentication clients could not be initialized");
    let appearance_settings =
        appearance_settings::AppearanceSettings::initialize(configuration_directory.clone())
            .expect("the appearance settings could not be initialized");
    let background_runtime =
        background_runtime::BackgroundRuntimeSettings::initialize(configuration_directory.clone())
            .expect("the background runtime settings could not be initialized");
    let desktop_integration = desktop_integration::DesktopIntegrationSettings::initialize(
        configuration_directory.clone(),
    )
    .expect("the desktop integration settings could not be initialized");
    let server_control = server_control::ServerControl::initialize(configuration_directory)
        .expect("the server control settings could not be initialized");

    let builder = tauri::Builder::default();
    #[cfg(windows)]
    let builder = builder
        // Must be registered first: a second launch restores the existing host.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = background_runtime::show_main_window(app, false);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_filter(|label| label == "main")
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        );

    builder
        .manage(
            health::HealthClients::new()
                .expect("the health-check HTTP clients could not be initialized"),
        )
        .manage(
            monitoring::GlancesClients::new(provider_auth.clone())
                .expect("the Glances HTTP clients could not be initialized"),
        )
        .manage(
            download_center::DownloadCenterClients::new()
                .expect("the download-center HTTP clients could not be initialized"),
        )
        .manage(provider_auth)
        .manage(
            service_discovery::ServiceDiscoveryClients::new()
                .expect("the service-discovery HTTP clients could not be initialized"),
        )
        .manage(appearance_settings)
        .manage(background_runtime)
        .manage(desktop_integration)
        .manage(server_control)
        .manage(service_webviews::ServiceWebviewRegistry::default())
        .manage(settings)
        .manage(catalog)
        .manage(desktop_widget_broker)
        .invoke_handler(tauri::generate_handler![
            health::check_service_health,
            monitoring::get_server_metrics,
            desktop_widgets::get_desktop_widget_snapshot,
            desktop_widgets::set_desktop_widget_visibility,
            background_runtime::get_background_runtime_preferences,
            background_runtime::save_background_runtime_preferences,
            background_runtime::get_desktop_widget_runtime_state,
            background_runtime::disable_desktop_widget,
            desktop_integration::get_desktop_integration_settings,
            desktop_integration::save_desktop_integration_settings,
            desktop_integration::set_desktop_startup_enabled,
            server_control::get_server_control_snapshot,
            server_control::save_server_control_settings,
            server_control::refresh_server_control_status,
            server_control::prepare_server_control_action,
            server_control::confirm_server_control_action,
            server_control::cancel_server_control_operation,
            server_control::dismiss_server_control_confirmation,
            appearance_settings::get_appearance_settings,
            appearance_settings::save_appearance_preferences,
            appearance_settings::reset_appearance_preferences,
            appearance_settings::import_appearance_preferences,
            appearance_settings::export_appearance_preferences,
            service_settings::get_service_configuration,
            service_settings::save_service_configuration,
            service_settings::reset_service_configuration,
            service_settings::restore_service_configuration_backup,
            credential_vault::get_service_credential_statuses,
            credential_vault::set_service_credential,
            credential_vault::delete_service_credential,
            provider_auth::get_service_authentication_status,
            provider_auth::validate_service_authentication,
            service_discovery::discover_homarr_services,
            server_inventory::discover_server_inventory,
            download_center::get_download_center_snapshot,
            service_webviews::open_service_webview,
            service_webviews::activate_service_webview,
            service_webviews::hide_service_webviews,
            service_webviews::reconcile_service_webviews,
            service_webviews::close_service_webview,
            service_webviews::open_service_in_system_browser,
        ])
        .setup(|app| {
            background_runtime::setup(app)?;
            appearance_settings::apply_current_appearance(app.handle())?;
            desktop_notifications::setup(app.handle())?;
            server_control::setup(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            let app = window.app_handle().clone();
            let settings = app.state::<background_runtime::BackgroundRuntimeSettings>();
            let catalog = app.state::<service_webviews::ServiceCatalog>();
            let background_work_enabled = app
                .state::<desktop_integration::DesktopIntegrationSettings>()
                .background_notifications_enabled()
                || app
                    .state::<server_control::ServerControl>()
                    .has_active_operation();
            let should_background =
                match settings.should_close_to_tray(&catalog, background_work_enabled) {
                    Ok(should_background) => should_background,
                    Err(error) => {
                        api.prevent_close();
                        eprintln!(
                        "Personal Hub stayed open because its close policy is unavailable: {error}"
                    );
                        return;
                    }
                };

            if !should_background {
                app.exit(0);
                return;
            }

            api.prevent_close();
            if !background_runtime::begin_close_to_tray(&settings) {
                return;
            }

            let worker_app = app.clone();
            let worker_window = window.clone();
            let spawn_result = std::thread::Builder::new()
                .name("personal-hub-close-to-tray".into())
                .spawn(move || {
                    let registry = worker_app.state::<service_webviews::ServiceWebviewRegistry>();
                    let teardown_result = service_webviews::suspend_and_close_all_service_webviews(
                        &worker_app,
                        &registry,
                    );
                    let background_result = teardown_result.and_then(|_| {
                        worker_window.hide().map_err(|error| {
                            format!("The main window could not be hidden: {error}")
                        })
                    });

                    if let Err(error) = background_result {
                        // Resume service creation only after the main window is
                        // visible again. This preserves the invariant that a
                        // hidden Personal Hub cannot own a playing service
                        // renderer, even when native close or hide fails.
                        let unminimize_result = match worker_window.is_minimized() {
                            Ok(true) => worker_window.unminimize(),
                            Ok(false) => Ok(()),
                            Err(error) => Err(error),
                        };
                        if unminimize_result.and_then(|_| worker_window.show()).is_ok() {
                            let _ = service_webviews::resume_service_webviews(&registry);
                            let _ = worker_window.set_focus();
                            let _ = worker_window.emit("personal-hub://main-resumed", ());
                        }
                        eprintln!("Personal Hub stayed open: {error}");
                    }

                    worker_app
                        .state::<background_runtime::BackgroundRuntimeSettings>()
                        .end_close_to_tray();
                });

            if let Err(error) = spawn_result {
                settings.end_close_to_tray();
                let _ = window.show();
                let _ = window.set_focus();
                eprintln!("Personal Hub could not start its close-to-tray worker: {error}");
            }
        })
        .run(context)
        .expect("error while running tauri application");
}
