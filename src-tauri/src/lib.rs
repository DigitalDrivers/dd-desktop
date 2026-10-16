//! The Digital Drivers desktop shell. It loads the hosted interface and offers a small, fixed set of
//! native commands to it. Logic that does not need Tauri or Windows APIs lives in `dd-core`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::Mutex;
use std::time::SystemTime;

use dd_core::engineer::{self, laps::LocalLap, store::SessionHead, EngineerSession, EngineerTrace};
use dd_core::evo_memory::{self, LiveSnapshot};
use dd_core::garage;
use dd_core::laps::{self, LapRecorder, LapSummary};
use dd_core::links;
use dd_core::motec;
use dd_core::settings;
use dd_core::setups::{self, SetupError, SetupFile};
use dd_core::stamp;
use serde::Serialize;
use tauri::{Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Address of the hosted interface. `DD_PLATFORM_URL` overrides it for development.
const DEFAULT_PLATFORM_URL: &str = "https://digitaldrivers.club";

/// Appends a line to `dd-desktop.log` in the system temp folder, with the time in UTC. Drivers can send this file
/// to support.
fn log(message: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("dd-desktop.log");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {message}", stamp::utc_stamp(unix_now()));
    }
}

/// Seconds since 1970.
fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

/// Whether the bundled start page installs a newer release before the interface loads: the update center's
/// switch "App updates at start", per PC. With `on` it changes it; it answers what holds now. Since 0.18.0.
#[tauri::command]
fn update_at_start(app: tauri::AppHandle, on: Option<bool>) -> Result<bool, String> {
    let dir = app.path().app_config_dir().map_err(|_| "failed".to_string())?;
    if let Some(on) = on {
        settings::set_update_at_start(&dir, on).map_err(|error| {
            log(&format!("update_at_start failed: {error}"));
            "failed".to_string()
        })?;
        log(&format!("update_at_start -> {on}"));
    }
    Ok(settings::update_at_start(&dir))
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
/// Before the download, `signature` (since 0.20.0, from the platform's garage) must be the club's signature of
/// that SHA-256 as this car and version; without it nothing is downloaded. A version lower than the one the app
/// installed last is refused (`downgrade`), the same one already in the game is nothing to do. Refused while the game runs. Saved cars of that car that point at files the new version no longer has move
/// to `SavedCars\stale`, and a garage that selected any saved car of it selects a stock car (after a backup).
#[tauri::command]
async fn install_car(app: tauri::AppHandle, state: tauri::State<'_, PackageHashes>, id: String, path: String, sha256: String, signature: Option<String>) -> Result<(), String> {
    log(&format!("install_car requested: {id} ({sha256})"));
    let result = download_car(&app, &id, &path, &sha256, signature.as_deref()).await;
    match &result {
        Ok(retired) => log(&format!("install_car -> {id} installed, {retired} stale saved cars retired")),
        Err(error) => log(&format!("install_car failed: {error}")),
    }
    state.0.lock().unwrap().retain(|p, _| !p.ends_with(format!("{id}.kspkg")));
    result.map(|_| ()).map_err(|e| e.split(':').next().unwrap_or("failed").to_string())
}

/// How far the download of a car is, as the event `car-progress` tells the page. `total` is None when the server
/// names no length.
#[derive(Clone, Serialize)]
struct CarProgress {
    id: String,
    received: u64,
    total: Option<u64>,
}

async fn download_car(app: &tauri::AppHandle, id: &str, path: &str, sha256: &str, signature: Option<&str>) -> Result<usize, String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let version = match garage::package_version(id, path) {
        Some(version) if garage::is_car_id(id) && sha256.len() == 64 => version,
        _ => return Err("invalid-car".to_string()),
    };
    if !signature.is_some_and(|signature| garage::is_signed(garage::PACKAGE_KEY, signature, id, version, sha256)) {
        return Err(format!("signature: {id} version {version} is {}", if signature.is_some() { "not signed by the club" } else { "unsigned" }));
    }
    let user_dir = find_ac_evo_user_dir(app).ok_or("ac-evo-not-found")?;
    if ac_evo_running() {
        return Err("game-running".to_string());
    }
    let mods = user_dir.join("mods");
    fs::create_dir_all(&mods).map_err(|e| format!("failed: mods folder: {e}"))?;
    let target = mods.join(format!("{id}.kspkg"));
    let part = mods.join(format!("{id}.kspkg.part"));
    // Never a lower version than the one installed; the same one already in the game is nothing to do. The
    // signature checked above says which version a package with this SHA-256 is, so it is kept as installed.
    let config = app.path().app_config_dir().map_err(|e| format!("failed: config folder: {e}"))?;
    let installed = garage::installed_version(&config, id);
    let file = target.clone();
    let on_disk = target.exists() && tauri::async_runtime::spawn_blocking(move || sha256_hex(&file)).await.is_ok_and(|hash| hash.is_ok_and(|hash| hash == sha256));
    match garage::install_plan(installed, version, on_disk) {
        garage::InstallPlan::Downgrade { installed } => return Err(format!("downgrade: {id} version {version} is older than version {installed} on this PC")),
        garage::InstallPlan::NothingToDo => {
            log(&format!("install_car: {id} version {version} is already in the game"));
            if installed != Some(version) {
                if let Err(error) = garage::record_installed(&config, id, version) {
                    log(&format!("install_car: keeping the version of {id}: {error}"));
                }
            }
            return Ok(0);
        }
        garage::InstallPlan::Install => {}
    }

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
    // Ask for gzip explicitly: a proxy in front of the platform (Cloudflare) unpacks a gzip answer for a client
    // that does not say it takes gzip, and then sends the package as it is, without the header.
    let mut response = client.get(&url).header("accept-encoding", "gzip").send().await.map_err(|e| format!("download-failed: {e}"))?;
    log(&format!("install_car: {} {:?}", response.status(), response.headers().get("content-encoding")));
    if !response.status().is_success() {
        return Err(format!("download-failed: {}", response.status()));
    }
    // The platform sends the package gzipped; whether it still is when it arrives depends on what is in between,
    // so the first bytes decide: gzip's magic number (a package starts with zeros) is unpacked on the way, anything
    // else is written as it comes. The SHA-256 of what lands in the file decides either way.
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
    enum Sink<W: Write> {
        Gzip(flate2::write::GzDecoder<Hashing<W>>),
        Plain(Hashing<W>),
    }
    let file = fs::File::create(&part).map_err(|e| format!("failed: {e}"))?;
    let mut target_file = Some(Hashing { inner: std::io::BufWriter::new(file), hasher: Sha256::new() });
    let mut out: Option<Sink<_>> = None;
    // Bytes as they arrive (gzipped, as the length the server names counts them), for the page's progress bar.
    let total = response.content_length();
    let mut received: u64 = 0;
    let mut last_progress: Option<std::time::Instant> = None;
    let progress = |received: u64| {
        let _ = app.emit("car-progress", CarProgress { id: id.to_string(), received, total });
    };
    let written: Result<(), String> = async {
        while let Some(chunk) = response.chunk().await.map_err(|e| format!("download-failed: {e}"))? {
            received += chunk.len() as u64;
            if last_progress.is_none_or(|at| at.elapsed() >= Duration::from_millis(250)) {
                last_progress = Some(std::time::Instant::now());
                progress(received);
            }
            if out.is_none() {
                let hashing = target_file.take().expect("the file is taken once");
                let gzip = chunk.starts_with(&[0x1f, 0x8b]);
                log(&format!("install_car: {} on arrival", if gzip { "gzipped" } else { "unpacked" }));
                out = Some(if gzip { Sink::Gzip(flate2::write::GzDecoder::new(hashing)) } else { Sink::Plain(hashing) });
            }
            match out.as_mut().expect("set above") {
                Sink::Gzip(decoder) => decoder.write_all(&chunk),
                Sink::Plain(plain) => plain.write_all(&chunk),
            }
            .map_err(|e| format!("failed: {e}"))?;
        }
        progress(received);
        Ok(())
    }
    .await;
    let finished = written.and_then(|_| match out {
        Some(Sink::Gzip(decoder)) => decoder.finish().map_err(|e| format!("failed: {e}")),
        Some(Sink::Plain(plain)) => Ok(plain),
        None => Err("download-failed: empty answer".to_string()),
    });
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
    if let Err(error) = garage::record_installed(&config, id, version) {
        log(&format!("install_car: keeping the version of {id}: {error}"));
    }
    Ok(retire_saved_cars(&user_dir, id, &target))
}

/// After an update of `id`: every saved car of it that points at a file the new version no longer has moves to
/// `SavedCars\stale\<unix time>` in every profile (kept, never deleted; also when the package's file table
/// cannot be read), the others stay in the game's "My cars". A garage that selected any saved car of `id` is
/// pointed at a stock car (after a backup): the game loads only the selected car on start, and a new version
/// can break a saved car in ways the file check does not see (2026-10-05: after the GT3 and Clubsport updates a
/// tester's game crashed on start, `Protobuf ... CHECK failed: ... key not found:`, no file missing). Such a
/// saved car crashes the game when the driver picks it; `fix_game_start` then moves that one. Returns how many
/// moved.
fn retire_saved_cars(user_dir: &Path, id: &str, package: &Path) -> usize {
    use std::io::{Read, Seek, SeekFrom};
    let table = (|| -> std::io::Result<Vec<u8>> {
        let mut file = fs::File::open(package)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(garage::TABLE_SIZE as u64)))?;
        let mut table = Vec::with_capacity(garage::TABLE_SIZE);
        file.read_to_end(&mut table)?;
        Ok(table)
    })();
    let paths = table.ok().map(|table| garage::package_paths(&table));
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut retired = 0;
    for profile in fs::read_dir(user_dir.join("ProfileData")).into_iter().flatten().flatten() {
        let open = profile.path().join("OpenData");
        let saved = open.join("SavedCars");
        let stale = saved.join("stale").join(stamp.to_string());
        let garage_file = open.join("garage.drivergarage");
        let selected = fs::read(&garage_file).ok().and_then(|g| garage::selected_pguid(&g));
        for entry in fs::read_dir(&saved).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !entry.path().is_file() || !name.starts_with(&format!("{id}_")) {
                continue;
            }
            let is_selected = selected.is_some() && garage::pguid_of(&name) == selected;
            if is_selected {
                let _ = fs::copy(&garage_file, open.join("garage.drivergarage.bak"));
                let _ = fs::write(&garage_file, garage::RESCUED_GARAGE);
                log(&format!("the garage selected {name}: it selects the Porsche 992 GT3 Cup now (backup garage.drivergarage.bak)"));
            }
            let check = match (&paths, fs::read(entry.path())) {
                (Some(paths), Ok(data)) if garage::is_stale(&data, id, paths) => "stale",
                (Some(_), Ok(_)) => {
                    log(&format!("saved car {name} points only at files the new version has: it stays"));
                    continue;
                }
                _ => "unchecked",
            };
            let _ = fs::create_dir_all(&stale);
            if fs::rename(entry.path(), stale.join(&name)).is_err() {
                continue;
            }
            retired += 1;
            log(&format!("moved saved car {name} to SavedCars\\stale\\{stamp} (file check: {check})"));
        }
    }
    retired
}

/// What `fix_game_start` did, for the page.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GameStartFix {
    /// Garages (one per profile) that select the Porsche 992 GT3 Cup now.
    garages: usize,
    /// Saved cars of the club's cars moved to `SavedCars\stale\<unix time>`.
    moved: usize,
    /// What the newest game log says about why the game stopped, for support.
    log: Vec<String>,
}

/// One click for a game that no longer starts after an update of a club car. The game loads the selected car
/// on every start, so a saved car the new version cannot read crashes it every time. Every profile's garage
/// selects the Porsche 992 GT3 Cup again (after a backup), and the saved car it had selected, when it is one of
/// the club's cars (`ids`), moves to `SavedCars\stale\<unix time>`, never deleted; the driver's other saved
/// cars stay. With `all`, every saved car of the club's cars moves. Refused while the game runs.
#[tauri::command]
fn fix_game_start(app: tauri::AppHandle, ids: Vec<String>, all: Option<bool>) -> Result<GameStartFix, String> {
    if ids.is_empty() || ids.len() > 50 || !ids.iter().all(|id| garage::is_car_id(id)) {
        return Err("invalid-car".to_string());
    }
    let user_dir = find_ac_evo_user_dir(&app).ok_or("ac-evo-not-found")?;
    if ac_evo_running() {
        return Err("game-running".to_string());
    }
    Ok(fix_game_start_in(&user_dir, &ids, all.unwrap_or(false)))
}

fn fix_game_start_in(user_dir: &Path, ids: &[String], all: bool) -> GameStartFix {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut fix = GameStartFix { garages: 0, moved: 0, log: Vec::new() };
    for profile in fs::read_dir(user_dir.join("ProfileData")).into_iter().flatten().flatten() {
        let open = profile.path().join("OpenData");
        let garage_file = open.join("garage.drivergarage");
        let selected = fs::read(&garage_file).ok().and_then(|g| garage::selected_pguid(&g));
        if garage_file.is_file() {
            let _ = fs::copy(&garage_file, open.join(format!("garage.drivergarage.{stamp}.bak")));
            if fs::write(&garage_file, garage::RESCUED_GARAGE).is_ok() {
                fix.garages += 1;
            }
        }
        let saved = open.join("SavedCars");
        let stale = saved.join("stale").join(stamp.to_string());
        for entry in fs::read_dir(&saved).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let ours = ids.iter().any(|id| name.starts_with(&format!("{id}_")));
            let is_selected = selected.is_some() && garage::pguid_of(&name) == selected;
            if !entry.path().is_file() || !ours || !(all || is_selected) {
                continue;
            }
            let _ = fs::create_dir_all(&stale);
            if fs::rename(entry.path(), stale.join(&name)).is_ok() {
                fix.moved += 1;
            }
        }
    }
    let newest = fs::read_dir(user_dir.join("Logs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH));
    if let Some(bytes) = newest.and_then(|e| fs::read(e.path()).ok()) {
        let tail = &bytes[bytes.len().saturating_sub(2_000_000)..];
        fix.log = garage::crash_lines(&String::from_utf8_lossy(tail));
    }
    log(&format!("fix_game_start (all: {all}): {} garages select the Porsche 992 GT3 Cup, {} saved cars moved to SavedCars\\stale\\{stamp}; log: {:?}", fix.garages, fix.moved, fix.log));
    fix
}

/// What AC EVO's shared memory says about the running session: the track and every car the game knows with
/// its position, for the live map. None while the game is not driving live. The game publishes the pages
/// itself; the app only opens them for reading (creating them would stop the game from publishing).
#[tauri::command]
fn live_snapshot() -> Option<LiveSnapshot> {
    let graphics = read_shared_memory("Local\\acevo_pmf_graphics", evo_memory::GRAPHICS_SIZE)?;
    let statics = read_shared_memory("Local\\acevo_pmf_static", evo_memory::STATIC_SIZE)?;
    evo_memory::read(&graphics, &statics)
}

/// Laps finished since the page last took them, whether the sampler runs, and whether the page takes laps (it asks
/// only with the driver's consent on the platform; until then no lap is kept for it).
#[derive(Default)]
struct Laps {
    started: std::sync::atomic::AtomicBool,
    platform: std::sync::atomic::AtomicBool,
    finished: Mutex<Vec<LapSummary>>,
}

/// Laps kept while the page does not take them; the oldest go first.
const LAPS_KEPT: usize = 200;

/// The laps of every car finished since the last call, from AC EVO's shared memory. The first call starts
/// the sampler, so nothing is read for a driver whose page never asks (the platform asks only with consent) and who
/// did not switch on the race engineer.
#[tauri::command]
fn take_laps(app: tauri::AppHandle, state: tauri::State<'_, Laps>) -> Vec<LapSummary> {
    state.platform.store(true, std::sync::atomic::Ordering::SeqCst);
    start_sampler(&app, "take_laps");
    std::mem::take(&mut *state.finished.lock().unwrap())
}

/// Starts the lap sampler once: for the platform's lap summaries (`take_laps`) or for the race engineer.
fn start_sampler(app: &tauri::AppHandle, by: &str) {
    if !app.state::<Laps>().started.swap(true, std::sync::atomic::Ordering::SeqCst) {
        log(&format!("{by}: sampling AC EVO's shared memory every 100 ms"));
        let app = app.clone();
        std::thread::spawn(move || sample_laps(app));
    }
}

/// Samples the pages ten times a second for good; while the game is not running they are missing and skipped.
fn sample_laps(app: tauri::AppHandle) {
    let mut recorder = LapRecorder::new();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let Some(physics) = read_shared_memory("Local\\acevo_pmf_physics", laps::PHYSICS_SIZE) else { continue };
        let Some(graphics) = read_shared_memory("Local\\acevo_pmf_graphics", evo_memory::GRAPHICS_SIZE) else { continue };
        let Some(statics) = read_shared_memory("Local\\acevo_pmf_static", evo_memory::STATIC_SIZE) else { continue };
        let Some(mut lap) = recorder.sample(&graphics, &physics, &statics) else { continue };
        // The pages give the car's display name; which car it is, the game's log says. Laps of every car are kept
        // (since 0.23.0); only the club's cars have a package version.
        let display_name = std::mem::take(&mut lap.car);
        let game_log = newest_game_log(&app);
        let Some((id, preset)) = game_log.as_deref().and_then(laps::current_car) else {
            log(&format!("lap of {display_name} not kept: the game's log names no car"));
            continue;
        };
        if !laps::platform_takes_car(&id) {
            log(&format!("lap of {display_name} not kept: the platform does not take the car id {id:?}"));
            continue;
        }
        if laps::is_club_car(&id) {
            let config = app.path().app_config_dir().ok();
            let package = find_ac_evo_user_dir(&app).map(|dir| dir.join("mods").join(format!("{id}.kspkg")));
            lap.package_version = config.zip(package).and_then(|(config, package)| garage::installed_package_version(&config, &id, &package));
        }
        lap.setup_name = game_log.as_deref().and_then(laps::current_setup);
        lap.car = id;
        lap.preset = Some(preset).filter(|preset| preset.encode_utf16().count() <= laps::PRESET_MAX);
        log(&format!(
            "lap: {} ({display_name}, {:?}, version {:?}, setup {:?}) at {} {}, {} ms, valid {}, pit {}, brake bias {}, balance {} deg; first sample raw: pressure {:?}, core temp {:?}, ride height {:?}",
            lap.car, lap.preset, lap.package_version, lap.setup_name, lap.track, lap.layout, lap.lap_time_ms, lap.valid, lap.pit, lap.brake_bias, lap.balance_deg, lap.first.pressure, lap.first.core_temp, lap.first.ride_height
        ));
        if engineer_on(&app) {
            engineer_record(&app, &lap);
        }
        let state = app.state::<Laps>();
        if !state.platform.load(std::sync::atomic::Ordering::SeqCst) {
            continue;
        }
        let mut finished = state.finished.lock().unwrap();
        if finished.len() >= LAPS_KEPT {
            finished.remove(0);
        }
        finished.push(lap);
    }
}

/// The race engineer's folder in the app's config folder.
fn engineer_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(engineer::store::DIR))
}

/// Whether the driver switched on the race engineer on this PC.
fn engineer_on(app: &tauri::AppHandle) -> bool {
    app.path().app_config_dir().is_ok_and(|dir| settings::engineer(&dir))
}

/// Milliseconds since 1970.
fn unix_now_ms() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The race engineer's state: whether it evaluates, and the lap it appended last (read from the list once).
#[derive(Default)]
struct Engineer {
    evaluating: std::sync::atomic::AtomicBool,
    last_lap: Mutex<Option<Option<LocalLap>>>,
}

/// Appends a finished lap, with the clock of this moment, to the engineer's lap list.
fn engineer_record(app: &tauri::AppHandle, lap: &LapSummary) {
    let Some(dir) = engineer_dir(app) else { return };
    let state = app.state::<Engineer>();
    let mut last = state.last_lap.lock().unwrap();
    let previous = last.get_or_insert_with(|| engineer::laps::load(&dir).pop());
    let Some(entry) = engineer::laps::entry(previous.as_ref(), lap, unix_now_ms()) else { return };
    match engineer::laps::append(&dir, &entry) {
        Ok(()) => *previous = Some(entry),
        Err(error) => log(&format!("engineer: keeping the lap failed: {error}")),
    }
}

/// What `engineer_status` answers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineerStatus {
    enabled: bool,
    /// Laps in the engineer's lap list
    laps: u32,
    /// Stored sessions
    sessions: u32,
    evaluating: bool,
    /// When the app last evaluated a session, seconds since 1970, null before the first
    last_evaluated: Option<u64>,
}

#[tauri::command]
fn engineer_status(app: tauri::AppHandle, state: tauri::State<'_, Engineer>) -> Result<EngineerStatus, String> {
    let dir = engineer_dir(&app).ok_or("failed")?;
    let (laps, sessions, last_evaluated) = engineer::store::counts(&dir);
    Ok(EngineerStatus { enabled: engineer_on(&app), laps, sessions, evaluating: state.evaluating.load(std::sync::atomic::Ordering::SeqCst), last_evaluated })
}

/// The race engineer's switch on this PC ("Evaluate laps on this PC"). On, the lap sampler runs whenever the game
/// runs, without the platform's consent (nothing leaves the PC), and the engineer keeps its lap list. Answers
/// whether it is on.
#[tauri::command]
fn engineer_enable(app: tauri::AppHandle, on: bool) -> Result<bool, String> {
    let config = app.path().app_config_dir().map_err(|_| "failed")?;
    settings::set_engineer(&config, on).map_err(|error| {
        log(&format!("engineer_enable failed: {error}"));
        "failed".to_string()
    })?;
    log(&format!("engineer_enable -> {on}"));
    if on {
        start_sampler(&app, "engineer_enable");
    }
    Ok(on)
}

/// The stored sessions, the newest first.
#[tauri::command]
fn engineer_sessions(app: tauri::AppHandle) -> Result<Vec<SessionHead>, String> {
    Ok(engineer::store::heads(&engineer_dir(&app).ok_or("failed")?))
}

/// A stored session's report; null when there is none of that id.
#[tauri::command]
fn engineer_session(app: tauri::AppHandle, id: String) -> Result<Option<EngineerSession>, String> {
    Ok(engineer::store::session(&engineer_dir(&app).ok_or("failed")?, &id))
}

/// A stored session's curves (every 5 m of each lap); null when there are none of that id.
#[tauri::command]
fn engineer_traces(app: tauri::AppHandle, id: String) -> Result<Option<Vec<EngineerTrace>>, String> {
    Ok(engineer::store::traces(&engineer_dir(&app).ok_or("failed")?, &id))
}

/// What `engineer_evaluate` did: how many sessions it evaluated, and why it evaluated none (`running`: an
/// evaluation runs; `driving`: the car is out of its pit box) or null.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineerEvaluation {
    evaluated: u32,
    held: Option<&'static str>,
}

/// The event `engineer-evaluated` after each session.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineerEvaluated {
    id: String,
    laps: usize,
}

/// Evaluates the MoTeC sessions that ended and are not evaluated yet, now.
#[tauri::command]
async fn engineer_evaluate(app: tauri::AppHandle) -> Result<EngineerEvaluation, String> {
    tauri::async_runtime::spawn_blocking(move || evaluate_sessions(&app, true)).await.map_err(|_| "failed".to_string())?
}

/// Deletes everything of the race engineer on this PC (lap list, sessions, corner maps, references); the switch
/// stays as it is.
#[tauri::command]
fn engineer_forget(app: tauri::AppHandle, state: tauri::State<'_, Engineer>) -> Result<(), String> {
    engineer::store::forget(&engineer_dir(&app).ok_or("failed")?).map_err(|error| {
        log(&format!("engineer_forget failed: {error}"));
        "failed".to_string()
    })?;
    *state.last_lap.lock().unwrap() = None;
    log("engineer_forget -> deleted");
    Ok(())
}

/// How often the app evaluates ended sessions by itself while the switch is on.
const ENGINEER_EVERY: Duration = Duration::from_secs(5 * 60);

/// Evaluates ended sessions every 5 minutes, while the switch is on and the game is not driving.
fn engineer_loop(app: tauri::AppHandle) {
    loop {
        std::thread::sleep(ENGINEER_EVERY);
        if engineer_on(&app) {
            if let Err(error) = evaluate_sessions(&app, false) {
                log(&format!("engineer: evaluating failed: {error}"));
            }
        }
    }
}

/// The game's logs (`Logs\log-*.txt`) written at or after `modified` (seconds since 1970), the oldest first: the
/// first of them is the game run that wrote a file of that time. At most three.
fn game_logs_since(app: &tauri::AppHandle, modified: u64) -> Vec<String> {
    let Some(logs) = find_ac_evo_user_dir(app).map(|dir| dir.join("Logs")) else { return Vec::new() };
    let mut found: Vec<(SystemTime, PathBuf)> = fs::read_dir(logs)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("log-"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(at, _)| at.duration_since(SystemTime::UNIX_EPOCH).is_ok_and(|d| d.as_secs() >= modified))
        .collect();
    found.sort();
    found.into_iter().take(3).filter_map(|(_, path)| fs::read(path).ok()).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).collect()
}

/// Evaluates every MoTeC session that ended and is not evaluated as it is; never while the game drives (`manual`:
/// the car out of its pit box, else live driving). Each session is noted as evaluated, also one whose file does not
/// read, so a broken file is not read every 5 minutes; the page gets `engineer-evaluated` for each.
fn evaluate_sessions(app: &tauri::AppHandle, manual: bool) -> Result<EngineerEvaluation, String> {
    use std::sync::atomic::Ordering::SeqCst;
    let state = app.state::<Engineer>();
    if driving(manual) {
        return Ok(EngineerEvaluation { evaluated: 0, held: Some("driving") });
    }
    if state.evaluating.swap(true, SeqCst) {
        return Ok(EngineerEvaluation { evaluated: 0, held: Some("running") });
    }
    let result = (|| -> Result<EngineerEvaluation, String> {
        let dir = engineer_dir(app).ok_or("failed: config folder")?;
        let root = find_ac_evo_user_dir(app).ok_or("ac-evo-not-found")?.join(motec::MOTEC_DIR);
        let files = motec_files(&root);
        let evaluated = engineer::sessions::evaluated(&dir);
        let ready: Vec<motec::MotecFile> = engineer::sessions::ready(&files, &evaluated, unix_now(), ac_evo_running()).into_iter().cloned().collect();
        let laps = engineer::laps::load(&dir);
        let version = app.package_info().version.to_string();
        let mut done = 0;
        for file in ready {
            if driving(manual) {
                break;
            }
            let logs = game_logs_since(app, file.modified);
            let logs: Vec<&str> = logs.iter().map(String::as_str).collect();
            let input = engineer::evaluate::Input { name: &file.name, modified_ms: file.modified as i64 * 1000, logs: &logs, laps: &laps, app_version: &version, now_ms: unix_now_ms() };
            let path = file.name.split('/').fold(root.clone(), |path, segment| path.join(segment));
            match fs::File::open(&path).and_then(|mut ld| engineer::evaluate::run(&dir, &mut std::io::BufReader::new(&mut ld), &input)) {
                Ok(outcome) => {
                    log(&format!("engineer: {} evaluated as {} ({} laps, clock from the game's log: {}){}", file.name, outcome.id, outcome.laps, outcome.from_log, if outcome.dropped.is_empty() { String::new() } else { format!("; not evaluated: {}", outcome.dropped.join("; ")) }));
                    let _ = app.emit("engineer-evaluated", EngineerEvaluated { id: outcome.id, laps: outcome.laps });
                    done += 1;
                }
                Err(error) => log(&format!("engineer: {} does not read: {error}", file.name)),
            }
            engineer::sessions::record_evaluated(&dir, &file, unix_now()).map_err(|e| format!("failed: {e}"))?;
        }
        Ok(EngineerEvaluation { evaluated: done, held: None })
    })();
    state.evaluating.store(false, SeqCst);
    result
}

/// The newest of AC EVO's logs (`Saved Games\ACE\Logs\log-*.txt`, next to the setups), as text. The game may be
/// writing it; Rust opens files for shared reading and writing on Windows, so reading does not disturb it.
fn newest_game_log(app: &tauri::AppHandle) -> Option<String> {
    let logs = find_ac_evo_setups(app)?.parent()?.join("Logs");
    let newest = fs::read_dir(logs)
        .ok()?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with("log-") && name.ends_with(".txt")
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()?;
    fs::read(newest.1).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// What `upload_motec` did, for the page.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MotecUpload {
    /// The session uploaded by this call (`<car>/<track>/<file>.ld`), or that none was.
    uploaded: Option<String>,
    /// Sessions still to upload after this call.
    waiting: u32,
    /// A session is running, or the previous call still uploads: nothing was uploaded.
    busy: bool,
    /// Why nothing was uploaded: `running`, `nothing-waiting`, `paused`, `driving`, `quiet` (the
    /// automatic upload waits for 10 quiet minutes) or `writing` (the game is writing the file); null otherwise.
    held: Option<&'static str>,
}

/// Whether a call of `upload_motec` is uploading (the page asks every minute, and an upload can take longer), and
/// until when no upload starts: after a full quota or a file the platform found broken, every attempt would send up
/// to 8 MiB, or the whole file, again for nothing.
#[derive(Default)]
struct MotecUploading {
    running: std::sync::atomic::AtomicBool,
    paused_until: Mutex<Option<std::time::Instant>>,
    /// The last line the log got about why nothing went up, so it says it once until the reason changes.
    held: Mutex<Option<String>>,
    /// The session going up, for the page's progress line.
    progress: Mutex<Option<MotecProgress>>,
    /// Sessions the calls in a row took so far; a call that takes none ends the run.
    run_done: std::sync::atomic::AtomicU32,
}

/// How far the running upload is: the `session`th (from 1) of the `sessions` of this run (the calls in a row that
/// each take one), and how many bytes of its gzip went up (`sent` after each chunk; both 0 while it is gzipped).
#[derive(Clone, Copy, Serialize)]
struct MotecProgress {
    session: u32,
    sessions: u32,
    sent: u64,
    total: u64,
}

/// How long uploads pause after a full quota or a file the platform found broken.
const MOTEC_RETRY: Duration = Duration::from_secs(6 * 60 * 60);

/// The pause before every chunk of an upload, which leaves the line to the driver's other traffic.
const MOTEC_PAUSE: Duration = Duration::from_secs(1);

/// How long a MoTeC file must stay as it is before it is gzipped: the game may be writing it.
const MOTEC_SETTLE: Duration = Duration::from_secs(2);

/// Whether the car is driven; then no MoTeC file goes up. The automatic upload counts live driving, by hand
/// (`manual`) only the car out of its pit box: the game writes its files when the car is back in the box.
fn driving(manual: bool) -> bool {
    if manual {
        read_shared_memory("Local\\acevo_pmf_graphics", evo_memory::GRAPHICS_SIZE).is_some_and(|graphics| evo_memory::on_track(&graphics))
    } else {
        live_snapshot().is_some()
    }
}

/// Answers a call that sends nothing, and logs why when the reason is another than last time (always once after
/// the start).
fn motec_held(state: &MotecUploading, hold: motec::Hold, waiting: u32, manual: bool) -> MotecUpload {
    let mut line = format!("upload_motec{}: nothing goes up, {}", if manual { " by hand" } else { "" }, hold.reason());
    if hold == motec::Hold::Driving {
        let pit_box = read_shared_memory("Local\\acevo_pmf_graphics", evo_memory::GRAPHICS_SIZE).map(|graphics| graphics[evo_memory::IS_IN_PIT_BOX]);
        line.push_str(&format!("{} (is_in_pit_box {pit_box:?})", if manual { " and the car is out of its pit box" } else { "" }));
    }
    let mut last = state.held.lock().unwrap();
    if last.as_deref() != Some(line.as_str()) {
        log(&line);
        *last = Some(line);
    }
    MotecUpload { uploaded: None, waiting, busy: hold.busy(), held: Some(hold.code()) }
}

/// Uploads one of the game's MoTeC sessions (`Saved Games\ACE\MoTec`) to the platform at `base`, which must be
/// the platform this app shows, with the ticket the page got (since 0.23.0; the page asks only with the driver's
/// MoTeC consent). The oldest session that ended and is not uploaded yet goes up: its newest `.ld`, gzipped, in
/// 8 MiB chunks from where the platform is, then its `.ldx`. Nothing while the game drives live, or runs and wrote a MoTeC file in the last
/// 10 minutes; `manual` (the page's "Upload now") skips the 10 minutes, so a running session goes up
/// too, and goes up while the car stands in its pit box (it stops when the car leaves the box). Never a file the
/// game is writing: it must stay unchanged for 2 s and while it is gzipped. The app's
/// config folder keeps what went up and when (`motec-uploaded.json`).
#[tauri::command]
async fn upload_motec(app: tauri::AppHandle, state: tauri::State<'_, MotecUploading>, base: String, ticket: String, manual: Option<bool>) -> Result<MotecUpload, String> {
    use std::sync::atomic::Ordering::SeqCst;
    let platform = std::env::var("DD_PLATFORM_URL").unwrap_or_else(|_| DEFAULT_PLATFORM_URL.to_string());
    if base.trim_end_matches('/') != platform.trim_end_matches('/') {
        log(&format!("upload_motec refused: {base} is not the platform"));
        return Err("invalid-base".to_string());
    }
    let manual = manual.unwrap_or(false);
    if state.running.swap(true, SeqCst) {
        return Ok(motec_held(&state, motec::Hold::Running, 0, manual));
    }
    let result = upload_next_motec(&app, &state, &base, &ticket, manual).await;
    *state.progress.lock().unwrap() = None;
    if matches!(&result, Ok(MotecUpload { held: None, .. })) {
        state.run_done.fetch_add(1, SeqCst);
    } else {
        state.run_done.store(0, SeqCst);
    }
    state.running.store(false, SeqCst);
    if let Err(error) = &result {
        log(&format!("upload_motec failed: {error}"));
        if error.starts_with("motec-mismatch") {
            *state.paused_until.lock().unwrap() = Some(std::time::Instant::now() + MOTEC_RETRY);
        }
    }
    result.map_err(|e| e.split(':').next().unwrap_or("failed").to_string())
}

/// How the upload of one session ended, short of an error.
enum MotecOutcome {
    Uploaded,
    /// The platform had it completed already.
    AlreadyThere,
    /// Larger than the platform takes for one file: never uploaded.
    TooLarge,
    /// The driver's quota on the platform is full: tried again later.
    Quota,
    /// The game started driving: the rest goes up later.
    Driving,
    /// The file changed while the app looked at it: the game is writing it.
    Writing,
}

async fn upload_next_motec(app: &tauri::AppHandle, state: &MotecUploading, base: &str, ticket: &str, manual: bool) -> Result<MotecUpload, String> {
    let root = find_ac_evo_user_dir(app).ok_or("ac-evo-not-found")?.join(motec::MOTEC_DIR);
    let config = app.path().app_config_dir().map_err(|e| format!("failed: config folder: {e}"))?;
    let listed = root.clone();
    let (files, game_running) = tauri::async_runtime::spawn_blocking(move || (motec_files(&listed), ac_evo_running())).await.map_err(|e| format!("failed: {e}"))?;
    let now = unix_now();
    let done = motec::uploaded(&config);
    let pending = motec::pending(&files, &done, now);
    let waiting = pending.len() as u32;
    let paused = state.paused_until.lock().unwrap().is_some_and(|until| std::time::Instant::now() < until);
    if let Some(hold) = motec::hold(&files, pending.len(), now, game_running, driving(manual), paused, manual) {
        return Ok(motec_held(state, hold, waiting, manual));
    }
    let next = pending[0];
    let (name, size) = (next.name.clone(), next.size);
    let path = name.split('/').fold(root, |path, segment| path.join(segment));
    *state.held.lock().unwrap() = None;
    log(&format!("upload_motec{}: {name} ({size} bytes), {waiting} sessions waiting", if manual { " by hand" } else { "" }));
    let done_in_run = state.run_done.load(std::sync::atomic::Ordering::SeqCst);
    *state.progress.lock().unwrap() = Some(MotecProgress { session: done_in_run + 1, sessions: done_in_run + waiting, sent: 0, total: 0 });
    let (uploaded, at) = match upload_motec_file(state, &path, base, ticket, &name, size, manual).await? {
        MotecOutcome::Uploaded => (Some(name.clone()), Some(unix_now())),
        MotecOutcome::AlreadyThere => {
            log("upload_motec: the platform has it already");
            (Some(name.clone()), None)
        }
        MotecOutcome::TooLarge => {
            log("upload_motec: larger than the platform takes, it stays on this PC");
            (None, None)
        }
        MotecOutcome::Quota => {
            log("upload_motec: the quota on the platform is full, trying again in 6 hours");
            *state.paused_until.lock().unwrap() = Some(std::time::Instant::now() + MOTEC_RETRY);
            return Ok(motec_held(state, motec::Hold::Paused, waiting, manual));
        }
        MotecOutcome::Driving => return Ok(motec_held(state, motec::Hold::Driving, waiting, manual)),
        MotecOutcome::Writing => return Ok(motec_held(state, motec::Hold::Writing, waiting, manual)),
    };
    // A file too large for the platform is recorded too, so the next call takes the next session.
    motec::record_uploaded(&config, &name, size, at).map_err(|e| format!("failed: keeping {name} as uploaded: {e}"))?;
    if uploaded.is_some() {
        log(&format!("upload_motec -> {name} uploaded"));
    }
    Ok(MotecUpload { uploaded, waiting: waiting - 1, busy: false, held: None })
}

/// What the page shows next to "Upload now"; it asks every few seconds while it is open.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MotecStatus {
    /// Sessions still to upload, a running one included.
    waiting: u32,
    /// A call of `upload_motec` is uploading.
    uploading: bool,
    /// The car is out of its pit box in a live session: nothing goes up by hand.
    driving: bool,
    game_running: bool,
    /// When the app last uploaded a session (seconds since 1970), null before the first.
    last_upload: Option<u64>,
    /// How far the running upload is, null while none goes up (also while a call only looks).
    progress: Option<MotecProgress>,
}

/// The state of the MoTeC upload, without talking to the platform.
#[tauri::command]
async fn motec_status(app: tauri::AppHandle, state: tauri::State<'_, MotecUploading>) -> Result<MotecStatus, String> {
    let root = find_ac_evo_user_dir(&app).ok_or("ac-evo-not-found")?.join(motec::MOTEC_DIR);
    let config = app.path().app_config_dir().map_err(|_| "failed")?;
    let (files, game_running) = tauri::async_runtime::spawn_blocking(move || (motec_files(&root), ac_evo_running())).await.map_err(|_| "failed")?;
    let done = motec::uploaded(&config);
    Ok(MotecStatus {
        waiting: motec::pending(&files, &done, unix_now()).len() as u32,
        uploading: state.running.load(std::sync::atomic::Ordering::SeqCst),
        driving: driving(true),
        game_running,
        last_upload: done.iter().filter_map(|u| u.at).max(),
        progress: *state.progress.lock().unwrap(),
    })
}

/// The `.ld` files of the MoTeC folder (`<car>\<track>\<file>.ld`), named with forward slashes.
fn motec_files(root: &Path) -> Vec<motec::MotecFile> {
    let entries = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten().collect::<Vec<_>>();
    let mut files = Vec::new();
    for car in entries(root).into_iter().filter(|e| e.path().is_dir()) {
        for track in entries(&car.path()).into_iter().filter(|e| e.path().is_dir()) {
            for file in entries(&track.path()) {
                let (car, track, name) = (car.file_name(), track.file_name(), file.file_name());
                let (Some(car), Some(track), Some(name)) = (car.to_str(), track.to_str(), name.to_str()) else { continue };
                let Ok(meta) = file.metadata() else { continue };
                if !meta.is_file() || !name.ends_with(".ld") {
                    continue;
                }
                let modified = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                files.push(motec::MotecFile { name: format!("{car}/{track}/{name}"), size: meta.len(), modified });
            }
        }
    }
    files
}

/// Gzips `source` into `target` (level 6, as `gzip`; the same bytes every time, so an upload resumes after a
/// restart); its size.
fn gzip_file(source: &Path, target: &Path) -> std::io::Result<u64> {
    use std::io::Write;
    let mut input = std::io::BufReader::new(fs::File::open(source)?);
    let mut encoder = flate2::write::GzEncoder::new(std::io::BufWriter::new(fs::File::create(target)?), flate2::Compression::default());
    std::io::copy(&mut input, &mut encoder)?;
    encoder.finish()?.flush()?;
    Ok(fs::metadata(target)?.len())
}

fn read_chunk(path: &Path, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut chunk = vec![0; len as usize];
    file.read_exact(&mut chunk)?;
    Ok(chunk)
}

/// An error of the platform as the command answers it: its `problem`, or `upload-failed`.
fn motec_refused(status: reqwest::StatusCode, body: &[u8], what: &str) -> String {
    let (problem, _) = motec::problem(body);
    format!("{}: {status} {what}", if problem.is_empty() { "upload-failed" } else { &problem })
}

/// The size and the time of the last write of `path`.
fn written(path: &Path) -> std::io::Result<(u64, SystemTime)> {
    let meta = fs::metadata(path)?;
    Ok((meta.len(), meta.modified()?))
}

/// Gzips the MoTeC file `source` into `target` unless the game is writing it: it must still have the `size` it was
/// listed with, stay unchanged for `MOTEC_SETTLE` and while it is gzipped. The gzip's size and SHA-256, or None.
fn gzip_settled(source: &Path, target: &Path, size: u64) -> std::io::Result<Option<(u64, String)>> {
    let before = written(source)?;
    if before.0 != size {
        return Ok(None);
    }
    std::thread::sleep(MOTEC_SETTLE);
    if written(source)? != before {
        return Ok(None);
    }
    let len = gzip_file(source, target)?;
    if written(source)? != before {
        return Ok(None);
    }
    Ok(Some((len, sha256_hex(target)?)))
}

async fn upload_motec_file(state: &MotecUploading, path: &Path, base: &str, ticket: &str, name: &str, size: u64, manual: bool) -> Result<MotecOutcome, String> {
    // Without a crypto provider building the client panics; the updater installs the same one when it runs.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("failed: {e}"))?;
    let url = |step: &str, offset: Option<u64>| motec::endpoint(base, step, ticket, name, offset).ok_or_else(|| "invalid-base".to_string());
    let failed = |e: reqwest::Error| format!("upload-failed: {e}");

    let response = client.get(url("upload", None)?).send().await.map_err(failed)?;
    let status = response.status();
    let body = response.bytes().await.map_err(failed)?;
    if !status.is_success() {
        return Err(motec_refused(status, &body, "asking how much the platform has"));
    }
    let received: motec::Received = serde_json::from_slice(&body).map_err(|e| format!("upload-failed: {e}"))?;
    if received.done {
        return Ok(MotecOutcome::AlreadyThere);
    }

    let gz = std::env::temp_dir().join("dd-motec-upload.ld.gz");
    let (source, target) = (path.to_path_buf(), gz.clone());
    let gzipped = tauri::async_runtime::spawn_blocking(move || gzip_settled(&source, &target, size)).await;
    let result = match gzipped {
        Ok(Ok(Some((len, sha256)))) => {
            log(&format!("upload_motec: gzipped to {len} bytes, the platform has {}", received.size));
            send_motec(state, &client, &url, &gz, len, &sha256, received.size, path, manual).await
        }
        Ok(Ok(None)) => Ok(MotecOutcome::Writing),
        Ok(Err(e)) => Err(format!("failed: gzip: {e}")),
        Err(e) => Err(format!("failed: {e}")),
    };
    let _ = fs::remove_file(&gz);
    result
}

/// Sends the gzipped file `gz` from `offset` on in chunks, then completes the upload with the `.ldx` next to `ld`;
/// stops when the car is driven (`manual`: see `driving`).
#[allow(clippy::too_many_arguments)]
async fn send_motec(
    state: &MotecUploading,
    client: &reqwest::Client,
    url: &impl Fn(&str, Option<u64>) -> Result<String, String>,
    gz: &Path,
    len: u64,
    sha256: &str,
    mut offset: u64,
    ld: &Path,
    manual: bool,
) -> Result<MotecOutcome, String> {
    let failed = |e: reqwest::Error| format!("upload-failed: {e}");
    let mut conflicts = 0;
    let sent = |offset: u64| {
        if let Some(progress) = state.progress.lock().unwrap().as_mut() {
            (progress.sent, progress.total) = (offset.min(len), len);
        }
    };
    sent(offset);
    while let Some((start, size)) = motec::chunk(offset, len) {
        if driving(manual) {
            return Ok(MotecOutcome::Driving);
        }
        let file = gz.to_path_buf();
        let chunk = tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(MOTEC_PAUSE);
            read_chunk(&file, start, size)
        })
        .await
        .map_err(|e| format!("failed: {e}"))?
        .map_err(|e| format!("failed: {e}"))?;
        let response = client.put(url("upload", Some(start))?).header("content-type", "application/octet-stream").body(chunk).send().await.map_err(failed)?;
        let status = response.status();
        let body = response.bytes().await.map_err(failed)?;
        if status.is_success() {
            offset = serde_json::from_slice::<motec::Received>(&body).map_err(|e| format!("upload-failed: {e}"))?.size;
            sent(offset);
            continue;
        }
        match (status.as_u16(), motec::problem(&body)) {
            // The platform has another size than the app thought (a chunk whose answer got lost): from there on.
            (409, (problem, Some(size))) if problem == "motec-offset" && conflicts < 3 => {
                conflicts += 1;
                log(&format!("upload_motec: the platform has {size} bytes, not {start}"));
                offset = size;
            }
            (413, (problem, _)) if problem == "motec-quota" => return Ok(MotecOutcome::Quota),
            (413, (problem, _)) if problem == "motec-too-large" => return Ok(MotecOutcome::TooLarge),
            _ => return Err(motec_refused(status, &body, &format!("at {start} of {len}"))),
        }
    }
    let ldx_path = ld.with_extension("ldx");
    let ldx = match fs::metadata(&ldx_path) {
        Ok(meta) if meta.len() <= motec::LDX_MAX => fs::read(&ldx_path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default(),
        _ => String::new(),
    };
    let complete = serde_json::json!({ "size": len, "sha256": sha256, "ldx": ldx });
    let response = client.post(url("complete", None)?).header("content-type", "application/json").body(complete.to_string()).send().await.map_err(failed)?;
    let status = response.status();
    if !status.is_success() {
        // On `motec-mismatch` the platform threw its part away: the next call starts this session over.
        let body = response.bytes().await.map_err(failed)?;
        return Err(motec_refused(status, &body, "completing"));
    }
    Ok(MotecOutcome::Uploaded)
}

/// A copy of a named shared memory page of another program, if it exists.
#[cfg(windows)]
fn read_shared_memory(name: &str, size: usize) -> Option<Vec<u8>> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenFileMappingA(access: u32, inherit: i32, name: *const u8) -> *mut c_void;
        fn MapViewOfFile(mapping: *mut c_void, access: u32, offset_high: u32, offset_low: u32, bytes: usize) -> *mut c_void;
        fn UnmapViewOfFile(address: *const c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const FILE_MAP_READ: u32 = 0x0004;
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: plain Win32 calls; the view is `size` bytes long as the game creates it, copied before it is
    // unmapped, and every handle is closed on every path.
    unsafe {
        let mapping = OpenFileMappingA(FILE_MAP_READ, 0, name.as_ptr().cast());
        if mapping.is_null() {
            return None;
        }
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, size);
        let copy = (!view.is_null()).then(|| std::slice::from_raw_parts(view as *const u8, size).to_vec());
        if !view.is_null() {
            UnmapViewOfFile(view);
        }
        CloseHandle(mapping);
        copy
    }
}

#[cfg(not(windows))]
fn read_shared_memory(_name: &str, _size: usize) -> Option<Vec<u8>> {
    None
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
    /// SteamID64 of the account signed in to Steam on this PC. None while Steam is not running.
    steam_id: Option<String>,
    /// Folder of the Assetto Corsa EVO installation, if one was found (since 0.6.0).
    ac_evo_path: Option<String>,
    /// The folder Assetto Corsa EVO keeps the driver's car setups in, once the game has been started on this
    /// PC (since 0.6.0). The app can put the platform's setups there.
    ac_evo_setups_path: Option<String>,
}

#[tauri::command]
fn system_check(webview: tauri::Webview, app: tauri::AppHandle) -> SystemCheck {
    log(&format!("system_check requested by {}", webview.url().map(|u| u.to_string()).unwrap_or_default()));
    let steam_id = dd_core::steam::steam_id64(active_steam_user());
    let ac_evo = find_ac_evo();
    let ac_evo_setups = find_ac_evo_setups(&app);
    log(&format!("system_check -> steam account: {steam_id:?}, assetto corsa evo: {ac_evo:?}, its setups: {ac_evo_setups:?}"));
    SystemCheck {
        app_version: app.package_info().version.to_string(),
        steam_id,
        ac_evo_path: ac_evo.map(|p| p.display().to_string()),
        ac_evo_setups_path: ac_evo_setups.map(|p| p.display().to_string()),
    }
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

/// Removes setups the platform handed out from the game's setup folder, each only while it is still the file the
/// platform hands out (`sha256`); one the driver changed stays. Answers "removed", "changed" or "missing" for each
/// file in the order asked. Track and car folders the removal left empty go too.
#[tauri::command]
async fn remove_setups(webview: tauri::Webview, app: tauri::AppHandle, files: Vec<setups::HandedOutSetup>) -> Result<Vec<String>, String> {
    log(&format!("remove_setups requested by {}: {} files", webview.url().map(|u| u.to_string()).unwrap_or_default(), files.len()));
    if files.len() > 500 {
        return Err("too-many-files".to_string());
    }
    let removed = find_ac_evo_setups(&app).ok_or(SetupError::AcEvoNotFound).and_then(|dir| setups::remove(&dir, &files));
    match removed {
        Ok(removals) => {
            let count = |what: setups::Removal| removals.iter().filter(|r| **r == what).count();
            log(&format!(
                "remove_setups -> {} removed, {} changed, {} missing",
                count(setups::Removal::Removed), count(setups::Removal::Changed), count(setups::Removal::Missing)
            ));
            Ok(removals.into_iter().map(|r| r.code().to_string()).collect())
        }
        Err(error) => {
            log(&format!("remove_setups failed: {error:?}"));
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
        // First, as the plugin asks: a second start only brings the running app's window to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log("second start: focusing the running app");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
                .manage(PendingUpdate::default())
        .manage(PackageHashes::default())
        .manage(Laps::default())
        .manage(MotecUploading::default())
        .manage(Engineer::default())
        .setup(|app| {
            // The race engineer keeps its lap list whenever the game runs while its switch is on, and evaluates
            // ended sessions every 5 minutes.
            if engineer_on(app.handle()) {
                start_sampler(app.handle(), "engineer");
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || engineer_loop(handle));
            // The window is built here instead of by the configuration alone, so that links which open a new
            // window (a stream, a download) go to the browser: the app has no tabs, and without this they did
            // nothing at all.
            // The window itself stays on the platform and Steam's sign-in; any other page goes to the browser.
            let config = app.config().app.windows.first().cloned().ok_or("no window in tauri.conf.json")?;
            let platform = platform_url();
            tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_new_window(|url, _features| {
                    open_in_browser(url.as_str());
                    tauri::webview::NewWindowResponse::Deny
                })
                .on_navigation(move |url| {
                    if links::stays_in_app(url.as_str(), &platform) {
                        return true;
                    }
                    log(&format!("navigation kept out of the window: {url}"));
                    open_in_browser(url.as_str());
                    false
                })
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![platform_url, system_check, show_toast, setup_status, install_setup, remove_setups, update_check, update_install, update_at_start, launch_ac_evo, car_status, install_car, fix_game_start, live_snapshot, take_laps, upload_motec, motec_status, engineer_status, engineer_enable, engineer_sessions, engineer_session, engineer_traces, engineer_evaluate, engineer_forget])
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

    /// A profile whose garage selects the GT3 Cup's pguid, which the saved car `selected` carries here, next to
    /// a second saved car of ours and one of a Kunos car; every saved car points at `a.material` of dd_x.
    fn profile(tag: &str) -> (PathBuf, PathBuf, &'static str) {
        let user = std::env::temp_dir().join(format!("dd-{tag}-{}", std::process::id()));
        let open = user.join("ProfileData").join("p").join("OpenData");
        let saved = open.join("SavedCars");
        fs::create_dir_all(&saved).unwrap();
        fs::write(open.join("garage.drivergarage"), garage::RESCUED_GARAGE).unwrap();
        let selected = "dd_x_4F44A4BE-5BE3-3C37-2B05-1984457156AC.carfinalstatewithconsumable";
        for name in [selected, "dd_x_1.carfinalstatewithconsumable", "ks_y_2.carfinalstatewithconsumable"] {
            fs::write(saved.join(name), b"\x0a\x1ccontent\\cars\\dd_x\\a.material").unwrap();
        }
        (user, open, selected)
    }

    fn stale_names(saved: &Path) -> Vec<String> {
        let folders: Vec<_> = fs::read_dir(saved.join("stale")).unwrap().flatten().collect();
        assert_eq!(folders.len(), 1);
        let mut names: Vec<String> = fs::read_dir(folders[0].path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn an_update_moves_only_the_saved_cars_it_broke_and_hands_the_garage_to_a_stock_car() {
        let (user, open, selected) = profile("retire");
        let saved = open.join("SavedCars");
        // dd_x_1 also points at a part the new version does not have.
        fs::write(saved.join("dd_x_1.carfinalstatewithconsumable"), b"\x0a\x1ccontent\\cars\\dd_x\\a.material\x12\x2bcontent\\cars\\dd_x\\parts\\gone.compatiblepart").unwrap();
        // The package: a file that is only its table, with one entry.
        let path = b"content\\cars\\dd_x\\a.material";
        let mut entry = vec![0u8; garage::ENTRY];
        entry[..path.len()].copy_from_slice(path);
        let table: Vec<u8> = entry.iter().enumerate().map(|(i, b)| b ^ garage::KEY[i % 8]).collect();
        let package = user.join("dd_x.kspkg");
        fs::write(&package, table).unwrap();
        assert_eq!(retire_saved_cars(&user, "dd_x", &package), 1);
        assert_eq!(stale_names(&saved), vec!["dd_x_1.carfinalstatewithconsumable"]);
        assert!(saved.join(selected).is_file());
        assert!(saved.join("ks_y_2.carfinalstatewithconsumable").is_file());
        // The garage selected a saved car of the updated car: it selects the Porsche now, with a backup.
        assert!(open.join("garage.drivergarage.bak").is_file());
        assert_eq!(fs::read(open.join("garage.drivergarage")).unwrap(), garage::RESCUED_GARAGE);
        fs::remove_dir_all(&user).unwrap();
    }

    #[test]
    fn the_fix_moves_the_selected_saved_car_and_every_one_of_ours_only_when_asked() {
        let (user, open, selected) = profile("fix");
        let saved = open.join("SavedCars");
        let ids = vec!["dd_x".to_string()];
        let fix = fix_game_start_in(&user, &ids, false);
        assert_eq!((fix.garages, fix.moved), (1, 1));
        assert_eq!(stale_names(&saved), vec![selected]);
        assert!(saved.join("dd_x_1.carfinalstatewithconsumable").is_file());
        assert!(saved.join("ks_y_2.carfinalstatewithconsumable").is_file());
        assert_eq!(fs::read(open.join("garage.drivergarage")).unwrap(), garage::RESCUED_GARAGE);
        assert_eq!(fs::read_dir(&open).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".bak")).count(), 1);
        // The garage selects the Porsche now, which no saved car carries: a second click moves nothing more...
        assert_eq!(fix_game_start_in(&user, &ids, false).moved, 0);
        // ...unless asked for every saved car of ours.
        assert_eq!(fix_game_start_in(&user, &ids, true).moved, 1);
        assert!(!saved.join("dd_x_1.carfinalstatewithconsumable").exists());
        assert!(saved.join("ks_y_2.carfinalstatewithconsumable").is_file());
        fs::remove_dir_all(&user).unwrap();
    }

    #[test]
    fn lists_the_motec_files_by_car_and_track_and_gzips_them_the_same_every_time() {
        let root = std::env::temp_dir().join(format!("dd-motec-{}", std::process::id()));
        let track = root.join("BMW M4 GT3 Evo").join("NurburgringTouristenfahrten");
        fs::create_dir_all(&track).unwrap();
        let ld = track.join("BMW M4 GT3 Evo_preset_m4gt3_mech_1_NurburgringTouristenfahrten_Sat Oct  3 195443 2026.ld");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&ld, &data).unwrap();
        fs::write(ld.with_extension("ldx"), b"<xml/>").unwrap();
        fs::write(root.join("BMW M4 GT3 Evo").join("stray.ld"), b"").unwrap();
        let files = motec_files(&root);
        assert_eq!(files.len(), 1, "only <car>/<track>/<file>.ld");
        assert_eq!(files[0].name, "BMW M4 GT3 Evo/NurburgringTouristenfahrten/BMW M4 GT3 Evo_preset_m4gt3_mech_1_NurburgringTouristenfahrten_Sat Oct  3 195443 2026.ld");
        assert_eq!(files[0].size, 300_000);
        assert!(motec::is_upload_name(&files[0].name));

        let (a, b) = (root.join("a.gz"), root.join("b.gz"));
        let len = gzip_file(&ld, &a).unwrap();
        assert_eq!(gzip_file(&ld, &b).unwrap(), len);
        assert_eq!(sha256_hex(&a).unwrap(), sha256_hex(&b).unwrap(), "a resumed upload sends the same bytes");
        let mut unpacked = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(fs::File::open(&a).unwrap()), &mut unpacked).unwrap();
        assert_eq!(unpacked, data);
        assert_eq!(read_chunk(&a, 10, 5).unwrap(), fs::read(&a).unwrap()[10..15]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn gzips_a_motec_file_only_while_the_game_is_not_writing_it() {
        let root = std::env::temp_dir().join(format!("dd-motec-settle-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let (ld, gz) = (root.join("a.ld"), root.join("a.ld.gz"));
        fs::write(&ld, vec![7u8; 100_000]).unwrap();
        let (len, sha256) = gzip_settled(&ld, &gz, 100_000).unwrap().unwrap();
        assert_eq!((len, sha256), (fs::metadata(&gz).unwrap().len(), sha256_hex(&gz).unwrap()));
        assert!(gzip_settled(&ld, &gz, 90_000).unwrap().is_none(), "it changed since it was listed");
        // the game writes on a second later
        let writer = {
            let ld = ld.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                std::io::Write::write_all(&mut fs::OpenOptions::new().append(true).open(ld).unwrap(), b"lap").unwrap();
            })
        };
        assert!(gzip_settled(&ld, &gz, 100_000).unwrap().is_none());
        writer.join().unwrap();
        fs::remove_dir_all(&root).unwrap();
    }
}
