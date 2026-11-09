/// The Visual C++ runtime the voice's ONNX Runtime is built against, copied from the build tools into `vcruntime/`,
/// from where the installer puts it next to the exe (`tauri.windows.conf.json`). Without it the app does not start
/// on a PC that has no such runtime, and crashes on one that has an old one. The rest of the app links its C
/// runtime statically.
#[cfg(windows)]
fn copy_vc_runtime() {
    use std::path::PathBuf;
    use std::process::Command;
    const DLLS: [&str; 4] = ["msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll"];
    let target = PathBuf::from("vcruntime");
    if DLLS.iter().all(|dll| target.join(dll).is_file()) {
        return;
    }
    // Where the installed build tools keep the redistributable files: a developer prompt names the folder,
    // otherwise Visual Studio's own locator does.
    let redist = std::env::var_os("VCToolsRedistDir").map(PathBuf::from).or_else(|| {
        let vswhere = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
        let found = Command::new(vswhere).args(["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"]).output().ok()?;
        let versions = PathBuf::from(String::from_utf8_lossy(&found.stdout).trim()).join("VC").join("Redist").join("MSVC");
        let mut folders: Vec<PathBuf> = std::fs::read_dir(versions).ok()?.flatten().map(|entry| entry.path()).filter(|path| path.join("x64").is_dir()).collect();
        folders.sort();
        folders.pop()
    });
    let crt = redist.and_then(|redist| std::fs::read_dir(redist.join("x64")).ok()?.flatten().map(|entry| entry.path()).find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".CRT"))));
    let crt = crt.expect("the Visual C++ redistributable files (VC\\Redist\\MSVC\\<version>\\x64\\Microsoft.VC143.CRT) of the build tools were not found");
    std::fs::create_dir_all(&target).expect("vcruntime folder");
    for dll in DLLS {
        std::fs::copy(crt.join(dll), target.join(dll)).unwrap_or_else(|error| panic!("copying {dll} from {}: {error}", crt.display()));
    }
}

fn main() {
    #[cfg(windows)]
    copy_vc_runtime();
    // Declaring the app's commands turns them into permissions ("allow-system-check", ...). Without this,
    // pages loaded from a remote origin cannot call any command, and with it every origin only gets the
    // commands its capability file lists.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["platform_url", "system_check", "show_toast", "setup_status", "install_setup", "remove_setups", "update_check", "update_install", "update_at_start", "launch_ac_evo", "car_status", "install_car", "fix_game_start", "live_snapshot", "take_laps", "upload_motec", "motec_status", "engineer_status", "engineer_enable", "engineer_sessions", "engineer_session", "engineer_traces", "engineer_evaluate", "engineer_forget", "engineer_forget_session", "engineer_keep", "engineer_profile", "engineer_seen", "engineer_digests", "engineer_digests_sent", "coach_focus_get", "coach_focus_set", "recorder_start", "recorder_stop", "radio_status", "radio_set", "radio_devices", "radio_download", "radio_test", "radio_preview", "radio_names", "radio_corner_names"])),
    )
    .expect("failed to run tauri-build");
}
