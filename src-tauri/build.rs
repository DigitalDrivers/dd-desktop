fn main() {
    // Declaring the app's commands turns them into permissions ("allow-system-check", ...). Without this,
    // pages loaded from a remote origin cannot call any command, and with it every origin only gets the
    // commands its capability file lists.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["platform_url", "system_check", "show_toast", "setup_status", "install_setup", "remove_setups", "update_check", "update_install", "update_at_start", "launch_ac_evo", "car_status", "install_car", "fix_game_start", "live_snapshot", "take_laps", "upload_motec", "motec_status", "engineer_status", "engineer_enable", "engineer_sessions", "engineer_session", "engineer_traces", "engineer_evaluate", "engineer_forget", "engineer_forget_session", "engineer_keep", "engineer_profile", "engineer_seen", "engineer_digests", "engineer_digests_sent", "coach_focus_get", "coach_focus_set", "recorder_start", "recorder_stop", "radio_status", "radio_set", "radio_devices", "radio_download", "radio_test", "radio_preview", "radio_names", "replay_list", "replay_keep", "replay_delete", "replay_open_folder", "game_check"])),
    )
    .expect("failed to run tauri-build");
}
