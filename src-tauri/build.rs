fn main() {
    const COMMANDS: &[&str] = &[
        "check_service_health",
        "get_server_metrics",
        "get_desktop_widget_snapshot",
        "set_desktop_widget_visibility",
        "get_background_runtime_preferences",
        "save_background_runtime_preferences",
        "get_desktop_widget_runtime_state",
        "disable_desktop_widget",
        "get_appearance_settings",
        "save_appearance_preferences",
        "reset_appearance_preferences",
        "import_appearance_preferences",
        "export_appearance_preferences",
        "get_service_configuration",
        "save_service_configuration",
        "reset_service_configuration",
        "restore_service_configuration_backup",
        "get_service_credential_statuses",
        "set_service_credential",
        "delete_service_credential",
        "get_service_authentication_status",
        "validate_service_authentication",
        "open_service_webview",
        "activate_service_webview",
        "hide_service_webviews",
        "reconcile_service_webviews",
        "close_service_webview",
        "open_service_in_system_browser",
    ];

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));

    tauri_build::try_build(attributes).expect("failed to run the Tauri build script");
}
