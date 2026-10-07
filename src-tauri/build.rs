fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "load_config", "load_todos", "save_todos", "probe", "scheduled_task_status",
            "watchdog_status", "action_status", "start_service", "stop_service", "open_page",
        ]),
    )).expect("desktop capability build failed")
}
