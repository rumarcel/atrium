fn main() {
    const COMMANDS: &[&str] = &[
        "check_service_health",
        "get_server_metrics",
        "get_desktop_widget_snapshot",
        "set_desktop_widget_visibility",
        "open_service_webview",
        "activate_service_webview",
        "hide_service_webviews",
        "reconcile_service_webviews",
        "park_service_webview",
        "close_service_webview",
        "open_service_in_system_browser",
    ];

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));

    tauri_build::try_build(attributes).expect("failed to run the Tauri build script");
}
