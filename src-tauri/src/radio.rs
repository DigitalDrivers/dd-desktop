//! The team radio's sound: the output devices of this PC and the speaker that plays what the coach says on the one
//! the driver chose. What is said, and with which radio sound on it, is decided in `dd_core::radio`; this hands the
//! finished samples to Windows (kira over cpal's WASAPI). Other systems have no speaker: the app runs on Windows.

use serde::Serialize;

/// An output device as the settings' picker shows it; the id is what the settings keep.
#[derive(Debug, Serialize)]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
}

#[cfg(windows)]
mod sound {
    use std::str::FromStr;
    use std::sync::Arc;

    use kira::backend::cpal::cpal::traits::{DeviceTrait, HostTrait};
    use kira::backend::cpal::cpal::{self, DeviceId};
    use kira::backend::cpal::{CpalBackend, CpalBackendSettings};
    use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
    use kira::sound::PlaybackState;
    use kira::{AudioManager, AudioManagerSettings, Frame, Tween};

    use super::OutputDevice;

    pub fn devices() -> Vec<OutputDevice> {
        let Ok(devices) = cpal::default_host().output_devices() else { return Vec::new() };
        devices.filter_map(|device| Some(OutputDevice { id: device.id().ok()?.to_string(), name: device.description().ok()?.name().to_string() })).collect()
    }

    /// Plays one call at a time on the driver's device. The device is opened with the first call and again when
    /// the driver chooses another one.
    #[derive(Default)]
    pub struct Speaker {
        /// The open device, with the id it was asked for (None for the PC's default device)
        open: Option<(Option<String>, AudioManager<CpalBackend>)>,
        playing: Option<StaticSoundHandle>,
    }

    impl Speaker {
        /// Plays the samples (mono) at `volume` (0 to 1) on `device`, in place of what is playing. A device that is
        /// gone falls back to the PC's default device. Answers the name of the problem when no device plays.
        pub fn play(&mut self, samples: &[f32], rate: u32, volume: f32, device: Option<&str>) -> Result<(), String> {
            self.stop();
            if self.open.as_ref().is_none_or(|(open, _)| open.as_deref() != device) {
                self.open = None;
                let chosen = device.and_then(|id| DeviceId::from_str(id).ok()).and_then(|id| cpal::default_host().device_by_id(&id));
                let settings = AudioManagerSettings { backend_settings: CpalBackendSettings { device: chosen, config: None }, ..Default::default() };
                let manager = AudioManager::<CpalBackend>::new(settings).map_err(|error| format!("no-output-device: {error}"))?;
                self.open = Some((device.map(str::to_string), manager));
            }
            let frames: Arc<[Frame]> = samples.iter().map(|sample| Frame::from_mono(*sample)).collect();
            let decibels = if volume <= 0.0 { -60.0 } else { 20.0 * volume.min(1.0).log10() };
            let sound = StaticSoundData { sample_rate: rate, frames, settings: StaticSoundSettings::new().volume(decibels), slice: None };
            let (_, manager) = self.open.as_mut().expect("opened above");
            match manager.play(sound) {
                Ok(handle) => {
                    self.playing = Some(handle);
                    Ok(())
                }
                Err(error) => {
                    // the device may have gone: the next call opens one again
                    self.open = None;
                    Err(format!("play-failed: {error}"))
                }
            }
        }

        /// Cuts off what is playing, without a click.
        pub fn stop(&mut self) {
            if let Some(mut playing) = self.playing.take() {
                playing.stop(Tween::default());
            }
        }

        /// Whether a call is still being played.
        pub fn busy(&self) -> bool {
            self.playing.as_ref().is_some_and(|playing| playing.state() != PlaybackState::Stopped)
        }
    }
}

#[cfg(not(windows))]
mod sound {
    use super::OutputDevice;

    pub fn devices() -> Vec<OutputDevice> {
        Vec::new()
    }

    #[derive(Default)]
    pub struct Speaker;

    impl Speaker {
        pub fn play(&mut self, _samples: &[f32], _rate: u32, _volume: f32, _device: Option<&str>) -> Result<(), String> {
            Err("no-output-device: only on Windows".to_string())
        }

        pub fn stop(&mut self) {}

        pub fn busy(&self) -> bool {
            false
        }
    }
}

pub use sound::{devices, Speaker};

// ---------------------------------------------------------------------------------------------------------------
// The radio itself: a loop that reads the game's pages twenty times a second, lets the spotter and the queue of
// `dd_core::radio` decide what to say, and plays the clip the voice rendered for it with the radio sound on it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use dd_core::evo_memory;
use dd_core::laps;
use dd_core::radio::effect::{self, Shape};
use dd_core::radio::engineer::{self, Engineer, Group};
use dd_core::radio::frame::{self, CarId};
use dd_core::radio::live_tips::{self, LiveTips};
use dd_core::radio::queue::{self, Call, Next, Priority, Queue, Situation};
use dd_core::radio::settings::{self, Settings};
use dd_core::radio::spotter::{Config, Spotter};
use dd_core::radio::phrases;
use dd_voice::Engine;
use tauri::{Emitter, Manager};

use crate::log;

/// The threads the voice computes on: more bring nothing and take the game's (measured 2026-11-09).
const VOICE_THREADS: usize = 2;

/// A call with its sound, spoken beside the loop.
type Spoken = (Call, Arc<Vec<f32>>);

/// The key of the sentences the coach answers with.
const COACH_ANSWER: &str = "coach_answer";
/// How long an answer's sentence stays worth saying.
const ANSWER_EXPIRES_S: f64 = 30.0;

/// How far a download or a render is, as the events `radio-download` and `radio-library` tell the page.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// What the coach calls a driver on this server: the platform hands the names over by car.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverName {
    pub car_ids: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub respelling: Respelling,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Respelling {
    pub de: Option<String>,
    pub en: Option<String>,
}

impl DriverName {
    /// How the name is written for the voice of a language: the driver's respelling, else the name.
    fn spoken(&self, lang: &str) -> &str {
        match lang {
            "en" => self.respelling.en.as_deref(),
            _ => self.respelling.de.as_deref(),
        }
        .filter(|respelling| !respelling.is_empty())
        .unwrap_or(&self.name)
    }
}

/// The radio's state in the app.
#[derive(Default)]
pub struct Radio {
    settings: Mutex<Option<Settings>>,
    speaker: Mutex<Speaker>,
    /// The voice, loaded when it first has to speak and dropped when the radio is switched off
    engine: Mutex<Option<Engine>>,
    /// The speech recognition of the coach's push-to-talk, loaded with the first question
    listener: Mutex<Option<dd_voice::Listener>>,
    names: Mutex<HashMap<CarId, DriverName>>,
    /// Clips ready to play (trimmed, levelled), by their cache name
    clips: Mutex<HashMap<String, Arc<Vec<f32>>>>,
    /// The corners' names by track and layout, as the platform's page hands them over, by the race engineer's
    /// corner numbers
    corner_names: Mutex<HashMap<(String, String), std::collections::BTreeMap<u32, String>>>,
    /// Counts up when corner names arrive, so the live tips take them
    corner_names_version: std::sync::atomic::AtomicU32,
    looping: AtomicBool,
    /// When the loop began: the clock of the calls
    clock: OnceLock<Instant>,
    /// Where a sentence that was spoken beside the loop joins the queue
    spoken: Mutex<Option<std::sync::mpsc::Sender<Spoken>>>,
    /// What the coach's model gets about the car and the session, renewed every second
    context: Mutex<serde_json::Value>,
    /// The question the coach is answering, and the number of the last one asked
    asking: Mutex<Option<u64>>,
    questions: std::sync::atomic::AtomicU64,
    /// The sentences of the coach's answer on their way to the voice, in their order
    answers: Mutex<Option<std::sync::mpsc::Sender<(u64, String)>>>,
    rendering: Mutex<Option<Progress>>,
    downloading: Mutex<Option<Progress>>,
}

fn config_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
}

/// Where the model lies: with the app's local data (383 MB, not roaming). A development build takes
/// `DD_SUPERTONIC_MODEL` when it is set.
fn model_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Ok(dir) = std::env::var("DD_SUPERTONIC_MODEL") {
            return Some(PathBuf::from(dir));
        }
    }
    app.path().app_local_data_dir().ok().map(|dir| dir.join("radio").join("model"))
}

/// Where the speech recognition's model lies (74 MB). A development build takes `DD_WHISPER_MODEL` when it is set.
fn listen_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Ok(dir) = std::env::var("DD_WHISPER_MODEL") {
            return Some(PathBuf::from(dir));
        }
    }
    app.path().app_local_data_dir().ok().map(|dir| dir.join("radio").join("listen"))
}

fn listen_ready(app: &tauri::AppHandle) -> bool {
    listen_dir(app).is_some_and(|dir| dd_voice::verify_listener(&dir, false).is_ok())
}

/// Where the rendered clips lie.
fn clips_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_local_data_dir().ok().map(|dir| dir.join("radio").join("clips"))
}

fn model_ready(app: &tauri::AppHandle) -> bool {
    model_dir(app).is_some_and(|dir| dd_voice::verify(&dir, false).is_ok())
}

/// The settings on this PC; read from the file once, then kept.
fn current(app: &tauri::AppHandle) -> Settings {
    let radio = app.state::<Radio>();
    let mut settings = radio.settings.lock().unwrap();
    settings.get_or_insert_with(|| config_dir(app).map(|dir| settings::load(&dir)).unwrap_or_default()).clone()
}

fn params() -> &'static effect::Params {
    static PARAMS: OnceLock<effect::Params> = OnceLock::new();
    PARAMS.get_or_init(effect::params)
}

/// A car id as the server's log and the platform write it, back into its two halves.
fn parse_car_id(id: &str) -> Option<CarId> {
    let (a, b) = id.split_once('-')?;
    Some((u64::from_str_radix(a, 16).ok()?, u64::from_str_radix(b, 16).ok()?))
}

/// The named variant of a call, for a car whose driver the radio knows.
fn named_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "car_left" => "name_left",
        "car_right" => "name_right",
        "closing_fast" => "name_closing_fast",
        "close_behind" => "name_close_behind",
        "slipstream_behind" => "name_slipstream_behind",
        "in_tow" => "name_in_tow",
        _ => return None,
    })
}

/// The clip of `text` as the voice rendered it, ready to play; None while it is not rendered.
fn clip(app: &tauri::AppHandle, voice: &str, lang: &str, text: &str) -> Option<Arc<Vec<f32>>> {
    let radio = app.state::<Radio>();
    let name = phrases::cache_name(voice, lang, text);
    if let Some(clip) = radio.clips.lock().unwrap().get(&name) {
        return Some(clip.clone());
    }
    let dry = dd_voice::load_clip(&clips_dir(app)?, voice, lang, text)?;
    let clip = Arc::new(effect::prepare(&dry, Engine::RATE));
    radio.clips.lock().unwrap().insert(name, clip.clone());
    Some(clip)
}

/// Plays a ready clip as a transmission with the driver's volume, effect strength and device.
fn transmit(app: &tauri::AppHandle, settings: &Settings, clip: &[f32], shape: Shape) -> Result<(), String> {
    let samples = effect::transmit(clip, Engine::RATE, params(), f32::from(settings.effect) / 100.0, shape);
    app.state::<Radio>().speaker.lock().unwrap().play(&samples, Engine::RATE, f32::from(settings.volume) / 100.0, settings.device.as_deref())
}

/// Runs `job` with the voice, loading the model first when it is not loaded.
fn with_engine<T>(app: &tauri::AppHandle, job: impl FnOnce(&mut Engine) -> Result<T, String>) -> Result<T, String> {
    let radio = app.state::<Radio>();
    let mut engine = radio.engine.lock().unwrap();
    if engine.is_none() {
        let dir = model_dir(app).ok_or("failed")?;
        let started = Instant::now();
        *engine = Some(Engine::load(&dir, VOICE_THREADS).map_err(|error| {
            log(&format!("radio: loading the voice failed: {error:?}"));
            error.code()
        })?);
        log(&format!("radio: the voice is loaded ({} ms)", started.elapsed().as_millis()));
    }
    job(engine.as_mut().expect("loaded above"))
}

/// Speaks a sentence now (not from the library) and answers it ready to play.
fn speak(app: &tauri::AppHandle, settings: &Settings, text: &str) -> Result<Vec<f32>, String> {
    let started = Instant::now();
    let dry = with_engine(app, |engine| engine.speak(text, &settings.language, &settings.voice).map_err(|error| error.code()))?;
    log(&format!("radio: spoke {} characters in {} ms", text.chars().count(), started.elapsed().as_millis()));
    Ok(effect::prepare(&dry, Engine::RATE))
}

/// Renders, in the background, what the radio needs for the driver's voice and language and has not rendered yet:
/// the phrase library, then the named calls of every driver it knows. Runs again when the voice, the language or
/// the names changed meanwhile; does nothing while the radio is off or the model is not there.
pub fn prepare_voice(app: &tauri::AppHandle) {
    let radio = app.state::<Radio>();
    if !current(app).enabled || !model_ready(app) {
        return;
    }
    {
        let mut rendering = radio.rendering.lock().unwrap();
        if rendering.is_some() {
            return;
        }
        *rendering = Some(Progress { done: 0, total: 0 });
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let radio = app.state::<Radio>();
        let mut rendered_for: Option<(String, String, usize)> = None;
        loop {
            let settings = current(&app);
            let names: Vec<DriverName> = radio.names.lock().unwrap().values().cloned().collect();
            let round = (settings.voice.clone(), settings.language.clone(), names.len());
            if !settings.enabled || rendered_for.as_ref() == Some(&round) {
                break;
            }
            let Some(clips) = clips_dir(&app) else { break };
            let result = with_engine(&app, |engine| {
                let library = dd_voice::render_library(engine, &settings.voice, &settings.language, &clips, |done, total| {
                    let progress = Progress { done: done as u64, total: total as u64 };
                    *radio.rendering.lock().unwrap() = Some(progress);
                    let _ = app.emit("radio-library", progress);
                })
                .map_err(|error| error.code())?;
                let mut spoken: Vec<&str> = names.iter().map(|name| name.spoken(&settings.language)).collect();
                spoken.sort_unstable();
                spoken.dedup();
                let mut named = 0;
                for name in spoken {
                    match dd_voice::render_names(engine, &settings.voice, &settings.language, name, &clips) {
                        Ok(rendered) => named += rendered.rendered.len(),
                        // a name the voice cannot say stays unnamed: the coach says "car left"
                        Err(error) => log(&format!("radio: a driver's name is not rendered: {}", error.code())),
                    }
                }
                Ok((library, named))
            });
            match result {
                Ok((library, named)) => log(&format!(
                    "radio: library for {} {}: {} rendered, {} there already, {} named calls rendered, {:.1} s",
                    settings.voice,
                    settings.language,
                    library.rendered.len(),
                    library.cached,
                    named,
                    library.elapsed.as_secs_f32()
                )),
                Err(error) => {
                    log(&format!("radio: rendering the library failed: {error}"));
                    break;
                }
            }
            rendered_for = Some(round);
        }
        *radio.rendering.lock().unwrap() = None;
        let _ = app.emit("radio-library", Progress { done: 0, total: 0 });
        // For tests without a settings page: a development build started with DD_RADIO_CHECK says the radio check
        // once its calls are rendered.
        if cfg!(debug_assertions) && std::env::var_os("DD_RADIO_CHECK").is_some() && rendered_for.is_some() {
            if let Err(error) = test(&app) {
                log(&format!("radio: the radio check failed: {error}"));
            }
        }
        // Also for tests: with DD_COACH_WAV (a 16-bit mono WAV) the recording is asked as if the driver had held
        // the coach's button and said it.
        if cfg!(debug_assertions) && rendered_for.is_some() {
            if let Some((samples, rate)) = std::env::var_os("DD_COACH_WAV").and_then(|path| std::fs::read(path).ok()).and_then(|wav| pcm16_mono(&wav)) {
                std::thread::sleep(Duration::from_secs(4));
                question(&app, samples, rate);
            }
        }
    });
}

/// The samples and the rate of a 16-bit mono WAV file.
fn pcm16_mono(wav: &[u8]) -> Option<(Vec<f32>, u32)> {
    let rate = u32::from_le_bytes(wav.get(24..28)?.try_into().ok()?);
    let data = wav.windows(4).position(|chunk| chunk == b"data")? + 8;
    (wav.get(22..24)? == [1, 0] && wav.get(34..36)? == [16, 0]).then(|| (wav[data.min(wav.len())..].chunks_exact(2).map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0).collect(), rate))
}

/// Starts the radio's loop once; it idles while the radio is off.
pub fn start(app: &tauri::AppHandle) {
    if !app.state::<Radio>().looping.swap(true, Ordering::SeqCst) {
        log("radio: reading AC EVO's shared memory every 50 ms while the team radio is on");
        let looping = app.clone();
        std::thread::spawn(move || run(looping));
        start_talk(app);
    }
}

/// The car and the track the game shows, as the bytes of their names: the engineer starts over when they change.
fn session_of(graphics: &[u8], statics: Option<&[u8]>) -> Vec<u8> {
    const CAR_MODEL: std::ops::Range<usize> = 3086..3119;
    const TRACK_AND_LAYOUT: std::ops::Range<usize> = 136..202;
    let mut session = graphics[CAR_MODEL].to_vec();
    session.extend_from_slice(statics.map_or(&[][..], |statics| &statics[TRACK_AND_LAYOUT]));
    session
}

/// The live tips of one car on one track, and where their reference lap is kept.
struct Tipping {
    tips: LiveTips,
    dir: PathBuf,
    car: String,
    preset: Option<String>,
    track: String,
    layout: String,
}

impl Tipping {
    /// For the car the game's log names and the track of the pages; None while either is unknown (the menus).
    fn of(app: &tauri::AppHandle, graphics: &[u8], lang: &str) -> Option<Self> {
        let statics = crate::read_shared_memory("Local\\acevo_pmf_static", evo_memory::STATIC_SIZE)?;
        let live = evo_memory::read(graphics, &statics)?;
        let (car, preset) = laps::current_car(&crate::newest_game_log(app)?)?;
        let preset = Some(preset).filter(|preset| preset.encode_utf16().count() <= laps::PRESET_MAX);
        let dir = config_dir(app)?.join(dd_core::engineer::store::DIR);
        let mut tips = LiveTips::new(live_tips::Config::for_layout(&live.track, &live.layout), lang);
        let reference = live_tips::load_reference(&dir, &car, preset.as_deref(), &live.track, &live.layout);
        log(&format!("radio: live tips for {car} at {} {}: {}", live.track, live.layout, if reference.is_some() { "with the best lap kept from before" } else { "the first clean lap becomes the reference" }));
        tips.set_reference(reference);
        // corners carry the race engineer's numbers once it has evaluated a session here
        let names = app.state::<Radio>().corner_names.lock().unwrap().get(&(live.track.clone(), live.layout.clone())).cloned().unwrap_or_default();
        tips.set_names(dd_core::engineer::corners::load(&dir, &live.track, &live.layout), names);
        Some(Self { tips, dir, car, preset, track: live.track, layout: live.layout })
    }

    /// Keeps a lap that just became the reference.
    fn keep_reference(&mut self) {
        if let Some(reference) = self.tips.take_new_reference() {
            match live_tips::save_reference(&self.dir, &self.car, self.preset.as_deref(), &self.track, &self.layout, reference) {
                Ok(()) => log(&format!("radio: live tips: a new reference lap for {} at {} {}", self.car, self.track, self.layout)),
                Err(error) => log(&format!("radio: live tips: keeping the reference lap failed: {error}")),
            }
        }
    }
}

fn run(app: tauri::AppHandle) {
    let radio = app.state::<Radio>();
    let started = *radio.clock.get_or_init(Instant::now);
    let mut spotter = Spotter::new(Config::default());
    let mut queue = Queue::new();
    let mut playing: Option<Call> = None;
    let mut picks: HashMap<&'static str, usize> = HashMap::new();
    let mut missing: Option<String> = None;
    // The engineer for the car, the track and the language of the moment.
    let mut engineer: Option<(Vec<u8>, String, Engineer)> = None;
    let mut session: Vec<u8> = Vec::new();
    let mut ticks = 0u32;
    // The live tips for the car and the track of the moment, with what names them in the engineer's folder.
    let mut tips: Option<(Vec<u8>, Tipping)> = None;
    let mut names_version = 0;
    let mut tips_tried = Instant::now() - Duration::from_secs(60);
    // Sentences the engineer builds are spoken beside the loop, so the spotter never waits for the voice; a
    // sentence joins the queue when its sound is there.
    let (spoken, sentences) = std::sync::mpsc::channel::<Spoken>();
    *radio.spoken.lock().unwrap() = Some(spoken.clone());
    // by the call's key and time: an answer has several sentences of one key
    let mut sentence_clips: HashMap<(&'static str, u64), (f64, Arc<Vec<f32>>)> = HashMap::new();
    queue.continuing.insert(COACH_ANSWER);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let settings = current(&app);
        if !settings.enabled {
            std::thread::sleep(Duration::from_millis(450));
            continue;
        }
        let Some(physics) = crate::read_shared_memory("Local\\acevo_pmf_physics", laps::PHYSICS_SIZE) else { continue };
        let Some(graphics) = crate::read_shared_memory("Local\\acevo_pmf_graphics", evo_memory::GRAPHICS_SIZE) else { continue };
        let now = started.elapsed().as_secs_f64();
        if ticks % 20 == 0 {
            session = session_of(&graphics, crate::read_shared_memory("Local\\acevo_pmf_static", evo_memory::STATIC_SIZE).as_deref());
        }
        ticks = ticks.wrapping_add(1);
        if engineer.as_ref().is_none_or(|(of, lang, _)| *of != session || *lang != settings.language) {
            engineer = Some((session.clone(), settings.language.clone(), Engineer::new(engineer::Config::default(), &settings.language)));
        }
        let (_, _, engineer) = engineer.as_mut().expect("made above");
        for say in engineer.step(now, &physics, &graphics) {
            let wanted = match say.group {
                Group::Fuel => settings.groups.fuel,
                Group::Laps => settings.groups.laps,
                Group::Car => settings.groups.car,
            };
            match say.text {
                _ if !wanted => {}
                None => queue.push(say.call),
                Some(text) => {
                    let (app, settings, spoken) = (app.clone(), settings.clone(), spoken.clone());
                    std::thread::spawn(move || match speak(&app, &settings, &text) {
                        Ok(clip) => drop(spoken.send((say.call, Arc::new(clip)))),
                        Err(error) => log(&format!("radio: {} is not said: {error}", say.call.key)),
                    });
                }
            }
        }
        for (call, clip) in sentences.try_iter() {
            sentence_clips.insert((call.key, call.at.to_bits()), (call.expires, clip));
            queue.push(call);
        }
        if ticks % 20 == 1 {
            sentence_clips.retain(|_, (expires, _)| *expires > now);
            // what the coach's model gets with a question
            let mut context = engineer.context();
            if let Some(live) = crate::read_shared_memory("Local\\acevo_pmf_static", evo_memory::STATIC_SIZE).and_then(|statics| evo_memory::read(&graphics, &statics)) {
                context["track"] = live.track.into();
                context["layout"] = live.layout.into();
                context["online"] = live.online.into();
            }
            if let Some((_, tipping)) = &tips {
                context["car"] = tipping.car.clone().into();
            }
            *radio.context.lock().unwrap() = context;
        }
        // the stint's summary when the car is back in its box
        if settings.coach.enabled && settings.coach.stints {
            if let Some(stint) = engineer.take_stint() {
                let mut context = radio.context.lock().unwrap().clone();
                context["stintLaps"] = stint["stintLaps"].clone();
                ask(&app, "stint", String::new(), &settings.language, context);
            }
        }
        // The spotter needs the driver's own car among the cars; the engineer speaks without.
        let frame = frame::read(now, &physics, &graphics);
        if let Some(frame) = &frame {
            for event in spotter.step(frame) {
                let call = queue::call(&event, now);
                let wanted = match call.priority {
                    Priority::Spotter => settings.groups.spotter,
                    Priority::Safety | Priority::Tactical => settings.groups.traffic,
                    _ => true,
                };
                if wanted {
                    queue.push(call);
                }
            }
        }
        // Live tips: one short sentence after a corner that cost time against the driver's best lap.
        if settings.live_tips {
            // corner names that arrived meanwhile: the tips start again with them (their reference lap is kept on disk)
            let version = radio.corner_names_version.load(Ordering::SeqCst);
            if version != names_version {
                names_version = version;
                tips = None;
                tips_tried = Instant::now() - Duration::from_secs(60);
            }
            if tips.as_ref().is_none_or(|(of, _)| *of != session) && tips_tried.elapsed() >= Duration::from_secs(5) {
                tips_tried = Instant::now();
                tips = Tipping::of(&app, &graphics, &settings.language).map(|tipping| (session.clone(), tipping));
            }
            if let (Some((of, tipping)), Some(frame)) = (tips.as_mut(), &frame) {
                if *of == session {
                    if let Some(tip) = tipping.tips.step(frame) {
                        let (app, settings, spoken) = (app.clone(), settings.clone(), spoken.clone());
                        std::thread::spawn(move || match speak(&app, &settings, &tip.text) {
                            Ok(clip) => drop(spoken.send((tip.call, Arc::new(clip)))),
                            Err(error) => log(&format!("radio: a live tip is not said: {error}")),
                        });
                    }
                    tipping.keep_reference();
                }
            }
        } else if tips.is_some() {
            tips = None;
        }
        if playing.is_some() && !radio.speaker.lock().unwrap().busy() {
            playing = None;
        }
        let situation = frame.as_ref().map_or(Situation { overlap: false, straight: false }, |frame| Situation::new(&frame.own, spotter.overlap()));
        let call = match queue.next(now, playing.as_ref(), situation) {
            Next::Play(call) | Next::Interrupt(call) => call,
            Next::Nothing => continue,
        };
        let pick = picks.entry(call.key).or_insert(0);
        *pick += 1;
        let (voice, lang) = (settings.voice.as_str(), settings.language.as_str());
        // a sentence spoken for this call; else with the driver's name when the radio knows the car and the named
        // call is rendered; else the plain call
        let sentence = sentence_clips.get(&(call.key, call.at.to_bits())).map(|(_, clip)| clip.clone());
        let named = || {
            let name = radio.names.lock().unwrap().get(&call.car?).cloned()?;
            let text = phrases::text(named_key(call.key)?, lang, Some(name.spoken(lang)), *pick)?;
            clip(&app, voice, lang, &text)
        };
        let ready = sentence.or_else(named).or_else(|| clip(&app, voice, lang, &phrases::text(call.key, lang, None, *pick)?));
        let Some(ready) = ready else {
            if missing.as_deref() != Some(call.key) {
                log(&format!("radio: {} is not said: its clip for {voice} {lang} is not rendered", call.key));
                missing = Some(call.key.to_string());
            }
            continue;
        };
        let shape = if matches!(call.priority, Priority::Spotter | Priority::Safety) { Shape::Spotter } else { Shape::Engineer };
        match transmit(&app, &settings, &ready, shape) {
            Ok(()) => {
                log(&format!("radio: {} ({} ms after the call was made)", call.key, ((started.elapsed().as_secs_f64() - call.at) * 1000.0).round()));
                playing = Some(call);
            }
            Err(error) => log(&format!("radio: {} is not said: {error}", call.key)),
        }
    }
}

/// A call made beside the loop, on the loop's clock.
fn call_now(app: &tauri::AppHandle, key: &'static str, priority: Priority, expires_s: f64) -> Call {
    let at = app.state::<Radio>().clock.get().map_or(0.0, |started| started.elapsed().as_secs_f64());
    Call { key, car: None, priority, at, expires: at + expires_s }
}

/// Puts a fixed phrase of the library into the queue from beside the loop.
fn say_phrase(app: &tauri::AppHandle, key: &'static str) {
    let settings = current(app);
    let picked = app.state::<Radio>().questions.load(Ordering::SeqCst) as usize;
    let ready = phrases::text(key, &settings.language, None, picked).and_then(|text| clip(app, &settings.voice, &settings.language, &text));
    match (ready, app.state::<Radio>().spoken.lock().unwrap().as_ref()) {
        (Some(ready), Some(spoken)) => drop(spoken.send((call_now(app, key, Priority::Tactical, ANSWER_EXPIRES_S), ready))),
        _ => log(&format!("radio: {key} is not said: not rendered, or the radio does not run")),
    }
}

/// Hands a question to the platform's page, which asks the coach's model and answers with `coach_say`: the event
/// `coach-question` (`{ id, kind, text, language, context }`). A new question takes the place of one still being
/// answered.
fn ask(app: &tauri::AppHandle, kind: &str, text: String, language: &str, context: serde_json::Value) {
    let radio = app.state::<Radio>();
    let id = radio.questions.fetch_add(1, Ordering::SeqCst) + 1;
    *radio.asking.lock().unwrap() = Some(id);
    log(&format!("radio: coach question {id} ({kind}, {} characters)", text.chars().count()));
    let _ = app.emit("coach-question", serde_json::json!({ "id": id, "kind": kind, "text": text, "language": language, "context": context }));
}

/// The driver's question as the microphone took it while the button was held: an acknowledgement at once, then the
/// words from the recording (on this PC; the sound goes nowhere), then the question to the platform.
fn question(app: &tauri::AppHandle, samples: Vec<f32>, rate: u32) {
    let settings = current(app);
    say_phrase(app, "copy_moment");
    match listen(app, &samples, rate, &settings.language) {
        Ok(text) if text.chars().any(char::is_alphanumeric) => {
            let context = app.state::<Radio>().context.lock().unwrap().clone();
            ask(app, "question", text, &settings.language, context);
        }
        Ok(_) => say_phrase(app, "coach_not_understood"),
        Err(error) => {
            log(&format!("radio: the question was not understood: {error}"));
            say_phrase(app, "coach_failed");
        }
    }
}

/// The words of a recording, recognised on this PC (Whisper base on the CPU); empty for silence and noise.
fn listen(app: &tauri::AppHandle, samples: &[f32], rate: u32, language: &str) -> Result<String, String> {
    let radio = app.state::<Radio>();
    let mut listener = radio.listener.lock().unwrap();
    if listener.is_none() {
        let dir = listen_dir(app).ok_or("failed")?;
        *listener = Some(dd_voice::Listener::load(&dir, VOICE_THREADS).map_err(|error| error.code())?);
    }
    let started = Instant::now();
    let heard = dd_voice::resample(samples, rate);
    let text = listener.as_mut().expect("loaded above").transcribe(&heard, language).map_err(|error| error.code())?;
    log(&format!("radio: {:.1} s of speech became {} characters in {} ms", heard.len() as f32 / dd_voice::Listener::RATE as f32, text.chars().count(), started.elapsed().as_millis()));
    Ok(text)
}

/// Starts watching the coach's button once: while the radio and the coach are on, holding it opens the microphone.
fn start_talk(app: &tauri::AppHandle) {
    let (binding, live, asked) = (app.clone(), app.clone(), app.clone());
    crate::talk::watch(
        move || {
            let settings = current(&binding);
            (settings.enabled && settings.coach.enabled).then_some(())?;
            Some(crate::talk::Binding { button: settings.coach.button?, microphone: settings.coach.microphone })
        },
        move || {
            // the microphone delivers: a short tone tells the driver to speak
            let settings = current(&live);
            let tone: Vec<f32> = (0..(Engine::RATE / 16)).map(|i| 0.25 * (i as f32 * 2.0 * std::f32::consts::PI * 880.0 / Engine::RATE as f32).sin() * (1.0 - i as f32 / (Engine::RATE / 16) as f32)).collect();
            let _ = live.state::<Radio>().speaker.lock().unwrap().play(&tone, Engine::RATE, f32::from(settings.volume) / 100.0, settings.device.as_deref());
        },
        move |samples, rate| {
            // on the watcher's thread: hand over at once, or the next press is not seen
            let app = asked.clone();
            std::thread::spawn(move || question(&app, samples, rate));
        },
    );
}

/// One sentence of the coach's answer to question `id`, from the platform's page: spoken and queued in the order
/// the sentences come. Sentences of a question that is no longer the one asked are dropped.
pub fn coach_say(app: &tauri::AppHandle, id: u64, text: String) {
    let radio = app.state::<Radio>();
    if *radio.asking.lock().unwrap() != Some(id) {
        return;
    }
    let text: String = text.trim().chars().take(400).collect();
    if text.is_empty() {
        return;
    }
    let mut answers = radio.answers.lock().unwrap();
    let sender = answers.get_or_insert_with(|| {
        let (sender, sentences) = std::sync::mpsc::channel::<(u64, String)>();
        let app = app.clone();
        std::thread::spawn(move || {
            for (id, text) in sentences {
                let radio = app.state::<Radio>();
                if *radio.asking.lock().unwrap() != Some(id) {
                    continue;
                }
                let settings = current(&app);
                match speak(&app, &settings, &phrases::spoken(&text, &settings.language)) {
                    Ok(clip) => {
                        if let Some(spoken) = radio.spoken.lock().unwrap().as_ref() {
                            let _ = spoken.send((call_now(&app, COACH_ANSWER, Priority::Tactical, ANSWER_EXPIRES_S), Arc::new(clip)));
                        }
                    }
                    Err(error) => log(&format!("radio: a sentence of the coach is not said: {error}")),
                }
            }
        });
        sender
    });
    let _ = sender.send((id, text));
}

/// The answer to question `id` is complete.
pub fn coach_done(id: u64) {
    log(&format!("radio: coach question {id} is answered"));
}

/// Question `id` got no answer: the coach says why with a fixed phrase.
pub fn coach_problem(app: &tauri::AppHandle, id: u64, problem: &str) {
    let radio = app.state::<Radio>();
    {
        let mut asking = radio.asking.lock().unwrap();
        if *asking != Some(id) {
            return;
        }
        *asking = None;
    }
    log(&format!("radio: coach question {id} failed: {problem}"));
    say_phrase(app, match problem {
        "tank-empty" => "coach_tank_empty",
        "not-available" | "too-many" => "coach_not_available",
        _ => "coach_failed",
    });
}

/// What `radio_status` answers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioStatus {
    settings: Settings,
    voices: &'static [&'static str],
    /// Whether the voice's model is on this PC
    model_ready: bool,
    /// Size of the model's download
    model_bytes: u64,
    downloading: Option<Progress>,
    rendering: Option<Progress>,
    /// Whether every call is rendered for the chosen voice and language
    library_ready: bool,
    /// The model's licence notice, to show where the voices are chosen
    licence: &'static str,
    /// Whether the speech recognition of the coach's push-to-talk is on this PC, the size of its download and its
    /// licence notice
    listen_ready: bool,
    listen_bytes: u64,
    listen_licence: &'static str,
}

pub fn status(app: &tauri::AppHandle) -> RadioStatus {
    let radio = app.state::<Radio>();
    let settings = current(app);
    let library_ready = clips_dir(app).is_some_and(|clips| {
        phrases::fixed_keys().iter().all(|key| phrases::wordings(key, &settings.language).iter().all(|text| dd_voice::is_rendered(&clips, &settings.voice, &settings.language, text)))
    });
    let downloading = *radio.downloading.lock().unwrap();
    let rendering = *radio.rendering.lock().unwrap();
    RadioStatus { settings, voices: Engine::voices(), model_ready: model_ready(app), model_bytes: dd_voice::MODEL_FILES.iter().map(|file| file.size).sum(), downloading, rendering, library_ready, licence: dd_voice::LICENCE_NOTICE, listen_ready: listen_ready(app), listen_bytes: dd_voice::LISTEN_MODEL_FILES.iter().map(|file| file.size).sum(), listen_licence: dd_voice::LISTEN_LICENCE_NOTICE }
}

/// Whether the driver switched the team radio on on this PC.
pub fn enabled(app: &tauri::AppHandle) -> bool {
    current(app).enabled
}

/// Changes the settings the page names and lets the radio follow: the loop starts with the first switch-on, the
/// voice renders what a new voice or language needs, and switching off frees the voice's memory.
pub fn set(app: &tauri::AppHandle, changes: &serde_json::Value) -> Result<RadioStatus, String> {
    let dir = config_dir(app).ok_or("failed")?;
    let changed = settings::change(&dir, changes).map_err(|error| {
        log(&format!("radio_set failed: {error}"));
        if error.kind() == std::io::ErrorKind::InvalidInput { "invalid" } else { "failed" }.to_string()
    })?;
    log(&format!("radio_set -> {}", serde_json::to_string(&changed).unwrap_or_default()));
    let radio = app.state::<Radio>();
    *radio.settings.lock().unwrap() = Some(changed.clone());
    if !changed.coach.enabled {
        *radio.listener.lock().unwrap() = None;
    }
    if changed.enabled {
        start(app);
        prepare_voice(app);
    } else {
        radio.speaker.lock().unwrap().stop();
        *radio.engine.lock().unwrap() = None;
        *radio.listener.lock().unwrap() = None;
        radio.clips.lock().unwrap().clear();
    }
    Ok(status(app))
}

/// Takes the names of the drivers on the driver's server from the platform's page and renders their named calls.
pub fn set_names(app: &tauri::AppHandle, names: Vec<DriverName>) {
    let radio = app.state::<Radio>();
    let mut by_car = HashMap::new();
    for name in names {
        for car in name.car_ids.iter().filter_map(|id| parse_car_id(id)) {
            by_car.insert(car, name.clone());
        }
    }
    let changed = {
        let mut known = radio.names.lock().unwrap();
        let changed = known.len() != by_car.len() || by_car.iter().any(|(car, name)| known.get(car).is_none_or(|k| k.name != name.name || k.respelling.de != name.respelling.de || k.respelling.en != name.respelling.en));
        *known = by_car;
        changed
    };
    if changed {
        log(&format!("radio: {} cars with a driver's name", radio.names.lock().unwrap().len()));
        prepare_voice(app);
    }
}

/// Takes the names of a track's corners from the platform's page, by the race engineer's corner numbers, so a live
/// tip says "Hatzenbach" instead of "corner four".
pub fn set_corner_names(app: &tauri::AppHandle, track: String, layout: String, names: std::collections::BTreeMap<u32, String>) {
    let radio = app.state::<Radio>();
    let names: std::collections::BTreeMap<u32, String> = names.into_iter().filter(|(_, name)| !name.is_empty() && name.chars().count() <= 60).take(400).collect();
    let mut known = radio.corner_names.lock().unwrap();
    let key = (track, layout);
    if known.get(&key) != Some(&names) {
        log(&format!("radio: {} corner names for {} {}", names.len(), key.0, key.1));
        known.insert(key, names);
        radio.corner_names_version.fetch_add(1, Ordering::SeqCst);
    }
}

/// Says the radio check on the driver's device, so the driver hears voice, volume and effect.
pub fn test(app: &tauri::AppHandle) -> Result<(), String> {
    let settings = current(app);
    let text = phrases::text("radio_check", &settings.language, None, 0).ok_or("failed")?;
    let ready = match clip(app, &settings.voice, &settings.language, &text) {
        Some(ready) => ready,
        None => Arc::new(speak(app, &settings, &text)?),
    };
    transmit(app, &settings, &ready, Shape::Engineer)
}

/// Says the driver's radio name in the two sentences of the preview, as the others will hear it. The name is the
/// driver's own input, checked again here as the platform checks it before it keeps one: letters, spaces, hyphens
/// and apostrophes, 2 to 30 characters.
pub fn preview(app: &tauri::AppHandle, name: &str) -> Result<(), String> {
    let name = name.trim();
    let length = name.chars().count();
    if !(2..=30).contains(&length) || !name.chars().all(|c| c.is_alphabetic() || matches!(c, ' ' | '-' | '\'' | '’')) {
        return Err("invalid-name".to_string());
    }
    let settings = current(app);
    let mut parts = Vec::new();
    for key in phrases::preview_keys() {
        let text = phrases::text(key, &settings.language, Some(name), 0).ok_or("failed")?;
        parts.push(speak(app, &settings, &text)?);
    }
    let parts: Vec<&[f32]> = parts.iter().map(Vec::as_slice).collect();
    transmit(app, &settings, &effect::join(&parts, Engine::RATE, 450), Shape::Engineer)
}

/// Downloads one file of the model into `dir` unless it is there with its size, and checks its SHA-256; tells
/// `received` every chunk's length.
async fn download_file(client: &reqwest::Client, file: &dd_voice::ModelFile, dir: &Path, mut received: impl FnMut(u64)) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let target = dir.join(file.path);
    if std::fs::metadata(&target).is_ok_and(|meta| meta.len() == file.size) {
        received(file.size);
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("failed: {error}"))?;
    }
    let part = dir.join(format!("{}.part", file.path));
    let mut response = client.get(file.url).send().await.map_err(|error| format!("download-failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("download-failed: {} for {}", response.status(), file.path));
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(&part).map_err(|error| format!("failed: {error}"))?);
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| format!("download-failed: {error}"))? {
        hasher.update(&chunk);
        out.write_all(&chunk).map_err(|error| format!("failed: {error}"))?;
        received(chunk.len() as u64);
    }
    out.flush().map_err(|error| format!("failed: {error}"))?;
    drop(out);
    let sha256: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if sha256 != file.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("checksum: {}", file.path));
    }
    std::fs::rename(&part, &target).map_err(|error| format!("failed: {error}"))
}

/// What `radio_download` and `radio_download_listen` fetch.
pub enum Model {
    Voice,
    Listener,
}

/// Downloads the voice's model (383 MB) or the speech recognition's (74 MB) from its pinned address at Hugging Face,
/// each file checked against its SHA-256, and tells the page how far it is with the event `radio-download`. A
/// download that broke off goes on with the files that are missing.
pub async fn download(app: &tauri::AppHandle, model: Model) -> Result<(), String> {
    let radio = app.state::<Radio>();
    let (files, dir, name): (&[dd_voice::ModelFile], _, _) = match model {
        Model::Voice => (&dd_voice::MODEL_FILES, model_dir(app), "the voice"),
        Model::Listener => (&dd_voice::LISTEN_MODEL_FILES, listen_dir(app), "the speech recognition"),
    };
    let dir = dir.ok_or("failed")?;
    let total: u64 = files.iter().map(|file| file.size).sum();
    {
        let mut downloading = radio.downloading.lock().unwrap();
        if downloading.is_some() {
            return Err("downloading".to_string());
        }
        *downloading = Some(Progress { done: 0, total });
    }
    log(&format!("radio_download: {name} into {}", dir.display()));
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let result = async {
        let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(60)).build().map_err(|error| format!("failed: {error}"))?;
        let mut done = 0u64;
        let mut told = Instant::now();
        for file in files {
            download_file(&client, file, &dir, |bytes| {
                done += bytes;
                let progress = Progress { done, total };
                *radio.downloading.lock().unwrap() = Some(progress);
                if told.elapsed() >= Duration::from_millis(250) {
                    told = Instant::now();
                    let _ = app.emit("radio-download", progress);
                }
            })
            .await?;
        }
        match model {
            Model::Voice => dd_voice::verify(&dir, false),
            Model::Listener => dd_voice::verify_listener(&dir, false),
        }
        .map_err(|error| error.code())
    }
    .await;
    *radio.downloading.lock().unwrap() = None;
    let _ = app.emit("radio-download", Progress { done: if result.is_ok() { total } else { 0 }, total });
    match &result {
        Ok(()) => {
            log(&format!("radio_download -> {name} is complete"));
            prepare_voice(app);
        }
        Err(error) => log(&format!("radio_download failed: {error}")),
    }
    result.map_err(|error| error.split(':').next().unwrap_or("failed").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_car_id_of_the_platform_goes_back_into_its_halves() {
        assert_eq!(parse_car_id("4f44a4be5be33c37-2b051984457156ac"), Some((0x4f44a4be5be33c37, 0x2b051984457156ac)));
        assert_eq!(parse_car_id(&evo_memory::car_id(1, 2)), Some((1, 2)));
        assert_eq!(parse_car_id("no"), None);
        assert_eq!(parse_car_id("12-xyz"), None);
    }

    #[test]
    fn a_driver_is_called_by_the_respelling_of_the_voice_s_language() {
        let name = DriverName { car_ids: vec![], name: "Jörn".into(), respelling: Respelling { de: Some("Jörrn".into()), en: None } };
        assert_eq!((name.spoken("de"), name.spoken("en")), ("Jörrn", "Jörn"));
        let empty = DriverName { car_ids: vec![], name: "Max".into(), respelling: Respelling { de: Some(String::new()), en: Some("Macks".into()) } };
        assert_eq!((empty.spoken("de"), empty.spoken("en")), ("Max", "Macks"));
    }

    #[test]
    fn every_call_that_can_carry_a_name_has_a_named_wording_in_the_library() {
        for key in ["car_left", "car_right", "closing_fast", "close_behind", "slipstream_behind", "in_tow"] {
            let named = named_key(key).unwrap();
            assert!(phrases::named_keys().contains(&named), "{named}");
            assert!(phrases::text(named, "de", Some("Max"), 0).unwrap().contains("Max"));
        }
        assert_eq!(named_key("clear_left"), None);
    }

    /// Needs the network: `cargo test -p dd-desktop downloads_a_small_model_file -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_a_small_model_file_and_checks_its_hash() {
        let dir = std::env::temp_dir().join(format!("dd-radio-download-{}", std::process::id()));
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::new();
        let file = dd_voice::MODEL_FILES.iter().find(|file| file.path == "onnx/tts.json").unwrap();
        let mut bytes = 0;
        tauri::async_runtime::block_on(download_file(&client, file, &dir, |n| bytes += n)).unwrap();
        assert_eq!(bytes, file.size);
        assert_eq!(std::fs::metadata(dir.join(file.path)).unwrap().len(), file.size);
        // a file with another hash is thrown away
        let wrong = dd_voice::ModelFile { sha256: "00", path: "LICENSE", ..*dd_voice::MODEL_FILES.iter().find(|file| file.path == "LICENSE").unwrap() };
        assert_eq!(tauri::async_runtime::block_on(download_file(&client, &wrong, &dir, |_| {})), Err("checksum: LICENSE".to_string()));
        assert!(!dir.join("LICENSE").exists() && !dir.join("LICENSE.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
