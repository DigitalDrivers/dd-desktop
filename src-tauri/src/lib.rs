//! The Digital Drivers desktop shell. It loads the hosted interface and offers a small, fixed set of
//! native commands to it. Logic that does not need Tauri or Windows APIs lives in `dd-core`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::Mutex;
use std::time::SystemTime;

use dd_core::evo_memory::{self, LiveSnapshot};
use dd_core::garage;
use dd_core::laps::{self, LapRecorder, LapSummary};
use dd_core::links;
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
    let written: Result<(), String> = async {
        while let Some(chunk) = response.chunk().await.map_err(|e| format!("download-failed: {e}"))? {
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
    Ok(retire_saved_cars(&user_dir, id, &target))
}

/// Moves every saved car of `id` to `SavedCars\stale\<unix time>` in every profile (kept, never deleted), and
/// points a garage that selected one of them at a stock car: the game loads the selected car on every start, and
/// a new version can break a saved car in ways the file check does not see (2026-10-05: after the GT3 and
/// Clubsport updates a tester's game crashed on start, `Protobuf ... CHECK failed: ... key not found:`). The
/// driver picks stage and livery again after an update. What the file check says about each one goes to the
/// log, until the cause is found. Returns how many moved.
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
                (Some(_), Ok(_)) => "not stale",
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
/// selects the Porsche 992 GT3 Cup again (after a backup), and the saved cars of the club's cars (`ids`) move to
/// `SavedCars\stale\<unix time>`, never deleted. Refused while the game runs.
#[tauri::command]
fn fix_game_start(app: tauri::AppHandle, ids: Vec<String>) -> Result<GameStartFix, String> {
    if ids.is_empty() || ids.len() > 50 || !ids.iter().all(|id| garage::is_car_id(id)) {
        return Err("invalid-car".to_string());
    }
    let user_dir = find_ac_evo_user_dir(&app).ok_or("ac-evo-not-found")?;
    if ac_evo_running() {
        return Err("game-running".to_string());
    }
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut fix = GameStartFix { garages: 0, moved: 0, log: Vec::new() };
    for profile in fs::read_dir(user_dir.join("ProfileData")).into_iter().flatten().flatten() {
        let open = profile.path().join("OpenData");
        let garage_file = open.join("garage.drivergarage");
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
            if !entry.path().is_file() || !ids.iter().any(|id| name.starts_with(&format!("{id}_"))) {
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
    log(&format!("fix_game_start: {} garages select the Porsche 992 GT3 Cup, {} saved cars moved to SavedCars\\stale\\{stamp}; log: {:?}", fix.garages, fix.moved, fix.log));
    Ok(fix)
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

/// Laps finished since the page last took them, and whether the sampler runs.
#[derive(Default)]
struct Laps {
    started: std::sync::atomic::AtomicBool,
    finished: Mutex<Vec<LapSummary>>,
}

/// Laps kept while the page does not take them; the oldest go first.
const LAPS_KEPT: usize = 200;

/// The laps of the club's cars finished since the last call, from AC EVO's shared memory. The first call starts
/// the sampler, so nothing is read for a driver whose page never asks (the platform asks only with consent).
#[tauri::command]
fn take_laps(app: tauri::AppHandle, state: tauri::State<'_, Laps>) -> Vec<LapSummary> {
    if !state.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
        log("take_laps: sampling AC EVO's shared memory every 100 ms");
        std::thread::spawn(move || sample_laps(app));
    }
    std::mem::take(&mut *state.finished.lock().unwrap())
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
        // The pages give the car's display name; which car it is, the game's log says. Only the club's cars' laps
        // are kept: nothing of any other car leaves the PC.
        let display_name = std::mem::take(&mut lap.car);
        match newest_game_log(&app).and_then(|log| laps::current_car(&log)) {
            Some((id, preset)) if laps::is_club_car(&id) => {
                lap.car = id;
                lap.preset = Some(preset);
            }
            other => {
                log(&format!("lap of {display_name} not kept: not a club car ({})", other.map_or("none".to_string(), |(id, _)| id)));
                continue;
            }
        }
        log(&format!(
            "lap: {} ({display_name}, {:?}) at {} {}, {} ms, valid {}, pit {}, brake bias {}, balance {} deg; first sample raw: pressure {:?}, core temp {:?}, ride height {:?}",
            lap.car, lap.preset, lap.track, lap.layout, lap.lap_time_ms, lap.valid, lap.pit, lap.brake_bias, lap.balance_deg, lap.first.pressure, lap.first.core_temp, lap.first.ride_height
        ));
        let state = app.state::<Laps>();
        let mut finished = state.finished.lock().unwrap();
        if finished.len() >= LAPS_KEPT {
            finished.remove(0);
        }
        finished.push(lap);
    }
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
        .invoke_handler(tauri::generate_handler![platform_url, system_check, show_toast, setup_status, install_setup, update_check, update_install, launch_ac_evo, car_status, install_car, fix_game_start, live_snapshot, take_laps])
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

    #[test]
    fn moves_every_saved_car_of_an_updated_car_aside() {
        let user = std::env::temp_dir().join(format!("dd-retire-{}", std::process::id()));
        let open = user.join("ProfileData").join("p").join("OpenData");
        let saved = open.join("SavedCars");
        fs::create_dir_all(&saved).unwrap();
        // The garage selects the GT3 Cup's pguid, which the first saved car carries here.
        fs::write(open.join("garage.drivergarage"), garage::RESCUED_GARAGE).unwrap();
        let selected = "dd_x_4F44A4BE-5BE3-3C37-2B05-1984457156AC.carfinalstatewithconsumable";
        for name in [selected, "dd_x_1.carfinalstatewithconsumable", "ks_y_2.carfinalstatewithconsumable"] {
            fs::write(saved.join(name), b"\x0a\x16content\\cars\\dd_x\\a.material").unwrap();
        }
        let package = user.join("dd_x.kspkg");
        fs::write(&package, b"").unwrap();
        assert_eq!(retire_saved_cars(&user, "dd_x", &package), 2);
        assert!(saved.join("ks_y_2.carfinalstatewithconsumable").is_file());
        let stale: Vec<_> = fs::read_dir(saved.join("stale")).unwrap().flatten().collect();
        assert_eq!(stale.len(), 1);
        assert!(stale[0].path().join(selected).is_file());
        assert!(stale[0].path().join("dd_x_1.carfinalstatewithconsumable").is_file());
        assert!(open.join("garage.drivergarage.bak").is_file());
        fs::remove_dir_all(&user).unwrap();
    }
}
