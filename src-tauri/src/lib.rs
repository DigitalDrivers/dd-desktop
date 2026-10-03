//! The Digital Drivers desktop shell. It loads the hosted interface and offers a small, fixed set of
//! native commands to it. Logic that does not need Tauri or Windows APIs lives in `dd-core`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::Mutex;
use std::time::SystemTime;

use dd_core::bot_race::{self, BotRaceResult, BotRaceTicket};
use dd_core::garage;
use dd_core::join::{self, JoinError, JoinTicket};
use dd_core::links;
use dd_core::scrutineering::{self, FileHash};
use dd_core::setups::{self, SetupError, SetupFile};
use serde::Serialize;
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Address of the hosted interface. `DD_PLATFORM_URL` overrides it for development.
const DEFAULT_PLATFORM_URL: &str = "https://digitaldrivers.club";

/// Appends a line to `dd-desktop.log` in the system temp folder. Drivers can send this file to support.
fn log(message: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("dd-desktop.log");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

/// The release `update_check` found, until `update_install` takes it.
#[derive(Default)]
struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAvailable {
    version: String,
}

/// The updater of this install, or None for an install that does not update itself: the packaged app is kept
/// up to date by the Store, and a build started from the target folder is the developer's own. Development
/// builds take the address of the update manifest from `DD_UPDATE_URL` (scripts/run-update-check.ps1).
fn updater_of(app: &tauri::AppHandle) -> Option<tauri_plugin_updater::Updater> {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("DD_UPDATE_URL") {
        return app.updater_builder().endpoints(vec![url.parse().ok()?]).ok()?.build().ok();
    }
    if package_app_id().is_some() || started_from_target_dir() {
        return None;
    }
    app.updater().ok()
}

/// Looks for a newer release of the app on GitHub. The bundled start page calls it before it loads the
/// hosted interface, and installs what it finds with `update_install`.
#[tauri::command]
async fn update_check(app: tauri::AppHandle, state: tauri::State<'_, PendingUpdate>) -> Result<Option<UpdateAvailable>, String> {
    let Some(updater) = updater_of(&app) else {
        log("update_check -> this install does not update itself");
        return Ok(None);
    };
    match updater.check().await {
        Ok(Some(update)) => {
            log(&format!("update_check -> {} is out, this is {}", update.version, update.current_version));
            let found = UpdateAvailable { version: update.version.clone() };
            *state.0.lock().unwrap() = Some(update);
            Ok(Some(found))
        }
        Ok(None) => {
            log("update_check -> up to date");
            Ok(None)
        }
        Err(error) => {
            log(&format!("update_check failed: {error}"));
            Err("failed".to_string())
        }
    }
}

/// Downloads the release `update_check` found, checks its signature and runs its installer. On Windows the
/// installer closes the app right away and starts the new version itself, so nothing after that is logged.
#[tauri::command]
async fn update_install(app: tauri::AppHandle, state: tauri::State<'_, PendingUpdate>) -> Result<(), String> {
    let update = state.0.lock().unwrap().take().ok_or_else(|| "no-update".to_string())?;
    log(&format!("update_install requested: {}", update.version));
    update.download_and_install(|_, _| {}, || log("update_install -> downloaded, checking the signature and handing over to the installer")).await.map_err(|error| {
        log(&format!("update_install failed: {error}"));
        "failed".to_string()
    })?;
    app.restart()
}

/// SHA-256 of the installed car packages, by path, as long as size and modification time are the same: hashing
/// 200 MB takes a second, and the garage page asks on every visit.
#[derive(Default)]
struct PackageHashes(Mutex<std::collections::HashMap<PathBuf, (u64, SystemTime, String)>>);

/// The game's folder in Saved Games (`ACE`), once the game has been started on this PC.
fn find_ac_evo_user_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    find_ac_evo_setups(app)?.parent().map(Path::to_path_buf)
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Which of the club's cars are installed: the SHA-256 of `mods\<id>.kspkg` for each id asked, in order, or
/// that it is not there. The platform knows which version that is.
#[tauri::command]
async fn car_status(app: tauri::AppHandle, state: tauri::State<'_, PackageHashes>, ids: Vec<String>) -> Result<Vec<Option<String>>, String> {
    if ids.len() > 50 || !ids.iter().all(|id| garage::is_car_id(id)) {
        return Err("invalid-car".to_string());
    }
    let Some(mods) = find_ac_evo_user_dir(&app).map(|dir| dir.join("mods")) else { return Err("ac-evo-not-found".to_string()) };
    let mut out = Vec::new();
    for id in ids {
        let path = mods.join(format!("{id}.kspkg"));
        let Ok(meta) = fs::metadata(&path) else {
            out.push(None);
            continue;
        };
        let stamp = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
        let cached = state.0.lock().unwrap().get(&path).filter(|c| (c.0, c.1) == stamp).map(|c| c.2.clone());
        let hash = match cached {
            Some(hash) => hash,
            None => {
                let file = path.clone();
                let hash = tauri::async_runtime::spawn_blocking(move || sha256_hex(&file)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
                state.0.lock().unwrap().insert(path, (stamp.0, stamp.1, hash.clone()));
                hash
            }
        };
        out.push(Some(hash));
    }
    Ok(out)
}

/// Whether Assetto Corsa EVO is running: a package must not be swapped under the game.
fn ac_evo_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("tasklist").args(["/FI", "IMAGENAME eq AssettoCorsaEVO.exe", "/NH"]).creation_flags(CREATE_NO_WINDOW).output();
        return out.map(|o| String::from_utf8_lossy(&o.stdout).contains("AssettoCorsaEVO.exe")).unwrap_or(false);
    }
    #[allow(unreachable_code)]
    false
}

/// Puts one of the club's cars into the game: downloads `path` (an address of the platform's garage, with the
/// ticket the page got), unpacks it on the way, checks its SHA-256 and only then replaces `mods\<id>.kspkg`.
/// Refused while the game runs. Saved cars of that car that point at parts the new version no longer has move
/// to `SavedCars\stale`, and a garage that selected one of them selects a stock car (after a backup).
#[tauri::command]
async fn install_car(app: tauri::AppHandle, state: tauri::State<'_, PackageHashes>, id: String, path: String, sha256: String) -> Result<(), String> {
    log(&format!("install_car requested: {id} ({sha256})"));
    let result = download_car(&app, &id, &path, &sha256).await;
    match &result {
        Ok(retired) => log(&format!("install_car -> {id} installed, {retired} stale saved cars retired")),
        Err(error) => log(&format!("install_car failed: {error}")),
    }
    state.0.lock().unwrap().retain(|p, _| !p.ends_with(format!("{id}.kspkg")));
    result.map(|_| ()).map_err(|e| e.split(':').next().unwrap_or("failed").to_string())
}

async fn download_car(app: &tauri::AppHandle, id: &str, path: &str, sha256: &str) -> Result<usize, String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    if !garage::is_car_id(id) || !path.starts_with(&format!("/api/garage/{id}/")) || sha256.len() != 64 {
        return Err("invalid-car".to_string());
    }
    let user_dir = find_ac_evo_user_dir(app).ok_or("ac-evo-not-found")?;
    if ac_evo_running() {
        return Err("game-running".to_string());
    }
    let mods = user_dir.join("mods");
    fs::create_dir_all(&mods).map_err(|e| format!("failed: mods folder: {e}"))?;
    let target = mods.join(format!("{id}.kspkg"));
    let part = mods.join(format!("{id}.kspkg.part"));

    let url = format!("{}{path}", platform_url());
    log(&format!("install_car: downloading {id}"));
    // Without a crypto provider building the client panics; the updater installs the same one when it runs.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("failed: {e}"))?;
    let mut response = client.get(&url).send().await.map_err(|e| format!("download-failed: {e}"))?;
    log(&format!("install_car: {} {:?}", response.status(), response.headers().get("content-encoding")));
    if !response.status().is_success() {
        return Err(format!("download-failed: {}", response.status()));
    }
    // The platform sends the package gzipped (content-encoding: gzip); it is unpacked into the file on the way.
    struct Hashing<W: Write> {
        inner: W,
        hasher: Sha256,
    }
    impl<W: Write> Write for Hashing<W> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(buf)?;
            self.hasher.update(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    let file = fs::File::create(&part).map_err(|e| format!("failed: {e}"))?;
    let mut out = flate2::write::GzDecoder::new(Hashing { inner: std::io::BufWriter::new(file), hasher: Sha256::new() });
    let written: Result<(), String> = async {
        while let Some(chunk) = response.chunk().await.map_err(|e| format!("download-failed: {e}"))? {
            out.write_all(&chunk).map_err(|e| format!("failed: {e}"))?;
        }
        Ok(())
    }
    .await;
    let finished = written.and_then(|_| out.finish().map_err(|e| format!("failed: {e}")));
    let mut sink = match finished {
        Ok(sink) => sink,
        Err(error) => {
            let _ = fs::remove_file(&part);
            return Err(error);
        }
    };
    sink.inner.flush().map_err(|e| format!("failed: {e}"))?;
    drop(sink.inner);
    let got: String = sink.hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    log(&format!("install_car: unpacked {id}, sha256 {got}"));
    if got != sha256 {
        let _ = fs::remove_file(&part);
        return Err(format!("checksum: {got}"));
    }
    if ac_evo_running() {
        let _ = fs::remove_file(&part);
        return Err("game-running".to_string());
    }
    fs::rename(&part, &target).map_err(|e| format!("failed: {e}"))?;
    Ok(retire_stale_cars(&user_dir, id, &target))
}

/// Moves the saved cars of `id` that point at files the installed package no longer has to `SavedCars\stale`,
/// in every profile, and points a garage that selected one of them at a stock car. Returns how many moved.
fn retire_stale_cars(user_dir: &Path, id: &str, package: &Path) -> usize {
    use std::io::{Read, Seek, SeekFrom};
    let table = (|| -> std::io::Result<Vec<u8>> {
        let mut file = fs::File::open(package)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(garage::TABLE_SIZE as u64)))?;
        let mut table = Vec::with_capacity(garage::TABLE_SIZE);
        file.read_to_end(&mut table)?;
        Ok(table)
    })();
    let Ok(table) = table else { return 0 };
    let paths = garage::package_paths(&table);
    let mut retired = 0;
    for profile in fs::read_dir(user_dir.join("ProfileData")).into_iter().flatten().flatten() {
        let open = profile.path().join("OpenData");
        let saved = open.join("SavedCars");
        let garage_file = open.join("garage.drivergarage");
        let selected = fs::read(&garage_file).ok().and_then(|g| garage::selected_pguid(&g));
        for entry in fs::read_dir(&saved).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(&format!("{id}_")) {
                continue;
            }
            let Ok(data) = fs::read(entry.path()) else { continue };
            if !garage::is_stale(&data, id, &paths) {
                continue;
            }
            let _ = fs::create_dir_all(saved.join("stale"));
            if fs::rename(entry.path(), saved.join("stale").join(&name)).is_err() {
                continue;
            }
            retired += 1;
            log(&format!("retired stale saved car {name}"));
            if selected.is_some() && garage::pguid_of(&name) == selected {
                let _ = fs::copy(&garage_file, open.join("garage.drivergarage.bak"));
                let _ = fs::write(&garage_file, garage::RESCUED_GARAGE);
                log("the garage selected it: it selects the Porsche 992 GT3 Cup now (backup garage.drivergarage.bak)");
            }
        }
    }
    retired
}

#[tauri::command]
fn platform_url() -> String {
    let url = std::env::var("DD_PLATFORM_URL").unwrap_or_else(|_| DEFAULT_PLATFORM_URL.to_string());
    log(&format!("platform_url -> {url}"));
    url
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemCheck {
    app_version: String,
    /// Folder of the Assetto Corsa installation, if one was found.
    assetto_corsa_path: Option<String>,
    /// Whether the app may write into that folder (needed to install content). None without a folder.
    assetto_corsa_writable: Option<bool>,
    /// SteamID64 of the account signed in to Steam on this PC. None while Steam is not running.
    steam_id: Option<String>,
    /// This app can start races against bots and report how they went (since 0.4.0).
    bot_races: bool,
    /// The program Content Manager registered for its `acmanager://` links, if it did and the file is still
    /// there (since 0.5.0). The platform links straight to it, or says what is missing.
    content_manager_path: Option<String>,
    /// Build of the Custom Shaders Patch in the game folder, if it is installed (since 0.5.0).
    csp_build: Option<u32>,
    /// Folder of the Assetto Corsa EVO installation, if one was found (since 0.6.0).
    ac_evo_path: Option<String>,
    /// The folder Assetto Corsa EVO keeps the driver's car setups in, once the game has been started on this
    /// PC (since 0.6.0). The app can put the platform's setups there.
    ac_evo_setups_path: Option<String>,
}

#[tauri::command]
fn system_check(webview: tauri::Webview, app: tauri::AppHandle) -> SystemCheck {
    log(&format!("system_check requested by {}", webview.url().map(|u| u.to_string()).unwrap_or_default()));

    let ac = find_assetto_corsa();
    let writable = ac.as_deref().map(can_write_into);
    let steam_id = join::steam_id64(active_steam_user());
    let content_manager = find_content_manager();
    let csp_build = ac.as_deref().and_then(scrutineering::csp_build);
    let ac_evo = find_ac_evo();
    let ac_evo_setups = find_ac_evo_setups(&app);
    log(&format!(
        "system_check -> assetto corsa: {ac:?}, writable: {writable:?}, steam account: {steam_id:?}, content manager: {content_manager:?}, CSP build {csp_build:?}, assetto corsa evo: {ac_evo:?}, its setups: {ac_evo_setups:?}"
    ));
    SystemCheck {
        app_version: app.package_info().version.to_string(),
        assetto_corsa_writable: writable,
        assetto_corsa_path: ac.map(|p| p.display().to_string()),
        steam_id,
        bot_races: true,
        content_manager_path: content_manager.map(|p| p.display().to_string()),
        csp_build,
        ac_evo_path: ac_evo.map(|p| p.display().to_string()),
        ac_evo_setups_path: ac_evo_setups.map(|p| p.display().to_string()),
    }
}

/// The race against bots the app has started, until its result has been fetched.
struct BotRaceRun {
    game: std::process::Child,
    started: SystemTime,
    result_file: PathBuf,
}

#[derive(Default)]
struct BotRaceState(Mutex<Option<BotRaceRun>>);

fn game_cfg_dir(app: &tauri::AppHandle) -> Result<PathBuf, JoinError> {
    Ok(app.path().document_dir().map_err(|e| JoinError::Failed(format!("documents folder: {e}")))?.join("Assetto Corsa"))
}

/// Starts a single-player race against the game's own AI: the driver's car against bots in the same car, on
/// the track and over the laps of the ticket. Like `join_race` it writes the game's `race.ini` and starts
/// `acs.exe`; how the race went is fetched with `bot_race_result` once the game has closed.
#[tauri::command]
async fn start_bot_race(webview: tauri::Webview, app: tauri::AppHandle, state: tauri::State<'_, BotRaceState>, ticket: BotRaceTicket) -> Result<(), String> {
    log(&format!(
        "start_bot_race requested by {}: {} on {} against {} bots at {} %, {} laps",
        webview.url().map(|u| u.to_string()).unwrap_or_default(), ticket.car_model, ticket.track, ticket.opponents, ticket.ai_level, ticket.laps
    ));
    let run = launch_bot_race(&app, &ticket).map_err(|error| {
        log(&format!("start_bot_race failed: {error:?}"));
        error.code()
    })?;
    *state.0.lock().unwrap() = Some(run);
    Ok(())
}

fn launch_bot_race(app: &tauri::AppHandle, ticket: &BotRaceTicket) -> Result<BotRaceRun, JoinError> {
    ticket.validate()?;
    let ac = find_assetto_corsa().ok_or(JoinError::AssettoCorsaNotFound)?;
    // Steam has to run for the game to start at all.
    join::steam_id64(active_steam_user()).ok_or(JoinError::SteamNotRunning)?;
    for (kind, name) in [("cars", &ticket.car_model), ("tracks", &ticket.track)] {
        if !ac.join("content").join(kind).join(name).is_dir() {
            return Err(JoinError::ContentMissing(format!("{kind}/{name}")));
        }
    }
    // The liveries the car has on this PC, by name: the driver gets the first, the bots the others in turn.
    let mut skins: Vec<String> = fs::read_dir(ac.join("content").join("cars").join(&ticket.car_model).join("skins"))
        .map(|dir| dir.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    skins.sort();

    let failed = |what: &str, error: std::io::Error| JoinError::Failed(format!("{what}: {error}"));
    let app_id = ac.join("steam_appid.txt");
    if !app_id.is_file() {
        fs::write(&app_id, join::STEAM_APP_ID).map_err(|e| failed("steam_appid.txt", e))?;
    }
    let documents = game_cfg_dir(app)?;
    fs::create_dir_all(documents.join("cfg")).map_err(|e| failed("cfg folder", e))?;
    let race_ini = documents.join("cfg").join("race.ini");
    let previous = fs::read(&race_ini).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    fs::write(&race_ini, bot_race::render_bot_race_ini(ticket, &skins, &previous)).map_err(|e| failed("race.ini", e))?;

    let started = SystemTime::now();
    let game = std::process::Command::new(ac.join("acs.exe")).current_dir(&ac).spawn().map_err(|e| failed("acs.exe", e))?;
    Ok(BotRaceRun { game, started, result_file: documents.join("out").join("race_out.json") })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BotRaceStatus {
    /// "none": no race was started; "running": the game is still open; "finished": the game has closed
    state: &'static str,
    /// With "finished": how the race went; None when the game left no race result (closed before the start)
    result: Option<BotRaceResult>,
}

/// How the race against bots started with `start_bot_race` stands. The result is handed out once.
#[tauri::command]
fn bot_race_result(state: tauri::State<'_, BotRaceState>) -> BotRaceStatus {
    let mut guard = state.0.lock().unwrap();
    let Some(run) = guard.as_mut() else { return BotRaceStatus { state: "none", result: None } };
    if matches!(run.game.try_wait(), Ok(None)) {
        return BotRaceStatus { state: "running", result: None };
    }
    // The game writes its result when it closes; only a file written after the start is this race's.
    let fresh = fs::metadata(&run.result_file).and_then(|m| m.modified()).map(|at| at >= run.started).unwrap_or(false);
    let result = if fresh { fs::read_to_string(&run.result_file).ok().and_then(|json| bot_race::parse_race_out(&json)) } else { None };
    log(&format!("bot_race_result -> game closed, result: {result:?}"));
    *guard = None;
    BotRaceStatus { state: "finished", result }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JoinStarted {
    /// The livery the race server has for the driver
    skin: String,
}

/// Starts the game on the race server of the ticket, the way a launcher does: asks the server for the
/// driver's slot, writes the game's `race.ini` for an online session there and starts `acs.exe`.
/// An error is a short code (see `JoinError::code`); the hosted interface has the words for it.
#[tauri::command]
async fn join_race(webview: tauri::Webview, app: tauri::AppHandle, ticket: JoinTicket) -> Result<JoinStarted, String> {
    log(&format!(
        "join_race requested by {}: {} on {}:{} ({})",
        webview.url().map(|u| u.to_string()).unwrap_or_default(),
        ticket.car_model,
        ticket.host,
        ticket.game_port,
        ticket.steam_id
    ));
    match start_race(&app, &ticket) {
        Ok(started) => {
            log(&format!("join_race -> game started, skin {}", started.skin));
            Ok(started)
        }
        Err(error) => {
            log(&format!("join_race failed: {error:?}"));
            Err(error.code())
        }
    }
}

fn start_race(app: &tauri::AppHandle, ticket: &JoinTicket) -> Result<JoinStarted, JoinError> {
    ticket.validate()?;
    let ac = find_assetto_corsa().ok_or(JoinError::AssettoCorsaNotFound)?;
    // The race server checks the game's Steam ticket against the slot: it has to be the same account.
    let local_account = join::steam_id64(active_steam_user()).ok_or(JoinError::SteamNotRunning)?;
    if local_account != ticket.steam_id {
        return Err(JoinError::WrongSteamAccount);
    }
    for (kind, name) in [("cars", &ticket.car_model), ("tracks", &ticket.track)] {
        if !ac.join("content").join(kind).join(name).is_dir() {
            return Err(JoinError::ContentMissing(format!("{kind}/{name}")));
        }
    }

    let entry_list = join::fetch_entry_list(&ticket.host, ticket.http_port, &ticket.steam_id, Duration::from_secs(5))?;
    let slot = join::slot_of(&entry_list, &ticket.car_model)?;

    let failed = |what: &str, error: std::io::Error| JoinError::Failed(format!("{what}: {error}"));
    // Without this file Steam starts the game's own launcher instead of the session.
    let app_id = ac.join("steam_appid.txt");
    if !app_id.is_file() {
        fs::write(&app_id, join::STEAM_APP_ID).map_err(|e| failed("steam_appid.txt", e))?;
    }
    let cfg = app.path().document_dir().map_err(|e| JoinError::Failed(format!("documents folder: {e}")))?.join("Assetto Corsa").join("cfg");
    fs::create_dir_all(&cfg).map_err(|e| failed("cfg folder", e))?;
    let race_ini = cfg.join("race.ini");
    let previous = fs::read(&race_ini).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    fs::write(&race_ini, join::render_race_ini(ticket, &slot, &previous)).map_err(|e| failed("race.ini", e))?;

    std::process::Command::new(ac.join("acs.exe")).current_dir(&ac).spawn().map_err(|e| failed("acs.exe", e))?;
    Ok(JoinStarted { skin: slot.skin })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScrutineeringReport {
    files: Vec<FileHash>,
    /// Build of the installed Custom Shaders Patch; None when it is not installed or switched off.
    csp_build: Option<u32>,
    app_version: String,
}

/// Technical scrutineering: reports the SHA-256 of the game files the hosted interface asks about (only
/// below the game's `content` and `system` folders) and the build of the Custom Shaders Patch. The platform
/// compares that with what the race server will check; the app itself judges nothing.
#[tauri::command]
async fn scrutineer(webview: tauri::Webview, app: tauri::AppHandle, files: Vec<String>) -> Result<ScrutineeringReport, String> {
    log(&format!("scrutineer requested by {}: {} files", webview.url().map(|u| u.to_string()).unwrap_or_default(), files.len()));
    if files.len() > 200 {
        return Err("too-many-files".to_string());
    }
    let ac = find_assetto_corsa().ok_or_else(|| JoinError::AssettoCorsaNotFound.code())?;
    let hashes = scrutineering::hash_files(&ac, &files).map_err(|path| {
        log(&format!("scrutineer refused the path {path}"));
        "invalid-path".to_string()
    })?;
    let csp_build = scrutineering::csp_build(&ac);
    log(&format!("scrutineer -> {} of {} files found, CSP build {csp_build:?}", hashes.iter().filter(|f| f.sha256.is_some()).count(), hashes.len()));
    Ok(ScrutineeringReport { files: hashes, csp_build, app_version: app.package_info().version.to_string() })
}

/// Says which of the setups the hosted interface asks about are in the game's setup folder: the SHA-256 of
/// each file in the order asked, or that it is not there. The platform knows whether that is the setup it
/// hands out, or one the driver changed.
#[tauri::command]
async fn setup_status(webview: tauri::Webview, app: tauri::AppHandle, files: Vec<SetupFile>) -> Result<Vec<Option<String>>, String> {
    log(&format!("setup_status requested by {}: {} files", webview.url().map(|u| u.to_string()).unwrap_or_default(), files.len()));
    if files.len() > 500 {
        return Err("too-many-files".to_string());
    }
    let found = find_ac_evo_setups(&app).ok_or(SetupError::AcEvoNotFound).and_then(|dir| setups::hash_setups(&dir, &files));
    match found {
        Ok(hashes) => {
            log(&format!("setup_status -> {} of {} files found", hashes.iter().flatten().count(), hashes.len()));
            Ok(hashes)
        }
        Err(error) => {
            log(&format!("setup_status failed: {error:?}"));
            Err(error.code())
        }
    }
}

/// Puts a car setup of the platform into the game's setup folder, where the game's setup screen lists it.
/// A file of that name the driver has changed stays as it is, unless `replace` asks for the original.
#[tauri::command]
async fn install_setup(webview: tauri::Webview, app: tauri::AppHandle, setup: SetupFile, data: Vec<u8>, replace: bool) -> Result<(), String> {
    log(&format!(
        "install_setup requested by {}: {} / {} / {} ({} bytes, replace: {replace})",
        webview.url().map(|u| u.to_string()).unwrap_or_default(), setup.car_folder, setup.track_folder, setup.name, data.len()
    ));
    let installed = find_ac_evo_setups(&app).ok_or(SetupError::AcEvoNotFound).and_then(|dir| setups::install(&dir, &setup, &data, replace));
    match installed {
        Ok(()) => {
            log("install_setup -> installed");
            Ok(())
        }
        Err(error) => {
            log(&format!("install_setup failed: {error:?}"));
            Err(error.code())
        }
    }
}

/// Shows a native notification (a Windows toast), also while the window is minimised. The hosted
/// interface calls it for new notifications of the signed-in driver. Plain text only, cut to a sane length.
#[tauri::command]
fn show_toast(webview: tauri::Webview, app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    let title: String = title.chars().take(80).collect();
    let body: String = body.chars().take(300).collect();
    let app_id = toast_app_id(&app.config().identifier);
    log(&format!(
        "show_toast requested by {}: {title} | {body} (app id {app_id})",
        webview.url().map(|u| u.to_string()).unwrap_or_default()
    ));
    show_native_toast(&app_id, &title, &body).map_err(|error| {
        log(&format!("show_toast failed: {error}"));
        error
    })
}

/// Starts Assetto Corsa EVO through Steam. The game has no join link: the hosted interface puts the
/// server's `join:<host>:<port>` string on the clipboard first, and the driver presses the clipboard
/// button in the game's server list. The Steam URL goes through the shell like a clicked link.
#[tauri::command]
fn launch_ac_evo(webview: tauri::Webview) -> Result<(), String> {
    log(&format!("launch_ac_evo requested by {}", webview.url().map(|u| u.to_string()).unwrap_or_default()));
    find_ac_evo().ok_or_else(|| "ac-evo-not-found".to_string())?;
    open_url("steam://run/3058630").map_err(|error| {
        log(&format!("launch_ac_evo failed: {error}"));
        format!("failed: {error}")
    })
}

#[cfg(windows)]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn().map(|_| ())
}

#[cfg(not(windows))]
fn open_url(_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("only on Windows"))
}

/// The id Windows files our notifications under. It has to be one Windows knows, or the toast is
/// dropped without an error.
fn toast_app_id(identifier: &str) -> String {
    // Installed from the Store or as MSIX, the package decides the id.
    if let Some(id) = package_app_id() {
        return id;
    }
    // A build started straight from the target folder is not registered with Windows at all;
    // PowerShell's id makes the toast show anyway. An installed build is registered by its identifier.
    if started_from_target_dir() { POWERSHELL_APP_ID.to_string() } else { identifier.to_string() }
}

fn started_from_target_dir() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(is_cargo_target_dir)).unwrap_or(false)
}

const POWERSHELL_APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

fn is_cargo_target_dir(dir: &Path) -> bool {
    dir.ends_with(Path::new("target").join("debug")) || dir.ends_with(Path::new("target").join("release"))
}

/// The application user model id of the package this process runs in, if it runs in one.
#[cfg(windows)]
fn package_app_id() -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentApplicationUserModelId(length: *mut u32, id: *mut u16) -> i32;
    }
    let mut buffer = [0u16; 256];
    let mut length = buffer.len() as u32;
    // SAFETY: `buffer` outlives the call and `length` holds its size in characters, as the API asks.
    let status = unsafe { GetCurrentApplicationUserModelId(&mut length, buffer.as_mut_ptr()) };
    // 0 is success, the length then counts the terminating zero. Without a package the call
    // answers APPMODEL_ERROR_NO_APPLICATION (15703).
    (status == 0 && length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(not(windows))]
fn package_app_id() -> Option<String> {
    None
}

#[cfg(windows)]
fn show_native_toast(app_id: &str, title: &str, body: &str) -> Result<(), String> {
    tauri_winrt_notification::Toast::new(app_id).title(title).text1(body).show().map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn show_native_toast(_app_id: &str, _title: &str, _body: &str) -> Result<(), String> {
    Err("Notifications are only implemented on Windows".to_string())
}

/// Writes and removes a small probe file to learn whether the folder is writable.
fn can_write_into(dir: &Path) -> bool {
    let probe = dir.join("dd-desktop-write-probe.tmp");
    let written = fs::write(&probe, b"Digital Drivers write probe").is_ok();
    let _ = fs::remove_file(&probe);
    written
}

fn find_assetto_corsa() -> Option<PathBuf> {
    // Development builds only: a game folder made up for a check (scripts/run-scrutineering-check.ps1).
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("DD_ASSETTO_CORSA_DIR").map(PathBuf::from) {
        return dd_core::steam::is_assetto_corsa_dir(&dir).then_some(dir);
    }
    let steam = steam_root()?;
    let vdf = fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).ok()?;
    dd_core::steam::find_assetto_corsa(&dd_core::steam::library_paths(&vdf))
}

fn find_ac_evo() -> Option<PathBuf> {
    let steam = steam_root()?;
    let vdf = fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).ok()?;
    dd_core::steam::find_ac_evo(&dd_core::steam::library_paths(&vdf))
}

/// The folder Assetto Corsa EVO keeps the car setups in. The game makes its folder in the user's
/// `Saved Games` when it first starts; without that folder there is no game to put setups into.
fn find_ac_evo_setups(app: &tauri::AppHandle) -> Option<PathBuf> {
    // Development builds only: a folder made up for a check (scripts/run-setups-check.ps1).
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("DD_AC_EVO_USER_DIR").map(PathBuf::from) {
        return dir.is_dir().then(|| dir.join(setups::SETUPS_DIR));
    }
    let home = app.path().home_dir().ok()?;
    let game = setups::saved_games_dir(moved_saved_games().as_deref(), &home).join(setups::USER_DIR);
    game.is_dir().then(|| game.join(setups::SETUPS_DIR))
}

/// Where the user moved the `Saved Games` folder to; None for a folder that is where Windows made it.
#[cfg(windows)]
fn moved_saved_games() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    // The folder's id among Windows' known folders.
    const SAVED_GAMES: &str = "{4C5C32FF-BB9D-43B0-B5B4-2D72E54EAAA4}";
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .ok()?
        .get_value(SAVED_GAMES)
        .ok()
}

#[cfg(not(windows))]
fn moved_saved_games() -> Option<String> {
    None
}

#[cfg(windows)]
fn steam_root() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam").ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    Some(PathBuf::from(path))
}

#[cfg(not(windows))]
fn steam_root() -> Option<PathBuf> {
    None
}

/// The program registered for `acmanager://` links, as long as it is still there. Content Manager registers
/// itself for the current user when it first starts.
#[cfg(windows)]
fn find_content_manager() -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|root| {
        let command: String = RegKey::predef(root).open_subkey(r"Software\Classes\acmanager\shell\open\command").ok()?.get_value("").ok()?;
        links::protocol_handler_exe(&command).filter(|exe| exe.is_file())
    })
}

#[cfg(not(windows))]
fn find_content_manager() -> Option<PathBuf> {
    None
}

/// A page the interface opens in a new window goes to the driver's browser; anything else is refused.
fn open_in_browser(url: &str) {
    if !links::opens_in_browser(url) {
        log(&format!("new window refused: {url}"));
        return;
    }
    log(&format!("opening in the browser: {url}"));
    // What the shell does with a link: hand it to the default browser.
    #[cfg(windows)]
    if let Err(error) = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn() {
        log(&format!("opening in the browser failed: {error}"));
    }
}

/// Account id of the user signed in to the running Steam; 0 while Steam is not running.
#[cfg(windows)]
fn active_steam_user() -> u32 {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam\ActiveProcess")
        .and_then(|key| key.get_value::<u32, _>("ActiveUser"))
        .unwrap_or(0)
}

#[cfg(not(windows))]
fn active_steam_user() -> u32 {
    0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(BotRaceState::default())
        .manage(PendingUpdate::default())
        .manage(PackageHashes::default())
        .setup(|app| {
            // The window is built here instead of by the configuration alone, so that links which open a new
            // window (a stream, a download) go to the browser: the app has no tabs, and without this they did
            // nothing at all.
            let config = app.config().app.windows.first().cloned().ok_or("no window in tauri.conf.json")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_new_window(|url, _features| {
                    open_in_browser(url.as_str());
                    tauri::webview::NewWindowResponse::Deny
                })
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![platform_url, system_check, show_toast, join_race, scrutineer, start_bot_race, bot_race_result, setup_status, install_setup, update_check, update_install, launch_ac_evo, car_status, install_car])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_build_started_from_the_cargo_target_folder() {
        assert!(is_cargo_target_dir(&Path::new("projects").join("dd-desktop").join("target").join("debug")));
        assert!(is_cargo_target_dir(&Path::new("dd-desktop").join("target").join("release")));
        assert!(!is_cargo_target_dir(&Path::new("Program Files").join("Digital Drivers")));
        assert!(!is_cargo_target_dir(&Path::new("WindowsApps").join("DigitalDrivers.Desktop_0.2.0.0_x64__5sms3s9pdbqt0")));
    }
}
