fn main() {
    // Declaring the app's commands turns them into permissions ("allow-system-check", ...). Without this,
    // pages loaded from a remote origin cannot call any command, and with it every origin only gets the
    // commands its capability file lists.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["platform_url", "system_check", "show_toast", "join_race", "scrutineer"])),
    )
    .expect("failed to run tauri-build");
}
