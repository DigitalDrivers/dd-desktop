//! The coach's push-to-talk: the driver holds a button on his wheel (or a key) to ask the coach, and the app records
//! the microphone only while it is held. The game has the focus meanwhile, so the buttons are read in the
//! background: game controllers through DirectInput 8 (nonexclusive, background; it sees DirectInput wheels such as
//! a Logitech G25, which XInput does not, and unlike Windows.Gaming.Input it does not need the focus), keys through
//! `GetAsyncKeyState`. The microphone is opened with the button going down and closed with it going up (cpal over
//! WASAPI, kira's re-export). Nothing is written to disk; what is recorded goes to `on_question` only. Other systems
//! have no buttons and no microphone here: the app runs on Windows.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dd_core::radio::settings::Button;
use serde::Serialize;

/// The shortest press that counts as a question; a shorter one is taken as an accidental touch.
pub const MIN_SECONDS: f32 = 0.3;
/// The longest question: the microphone closes after this even while the button is held.
pub const MAX_SECONDS: f32 = 20.0;
/// How often the watcher looks at the bound button.
pub const POLL: Duration = Duration::from_millis(15);
/// How often the watcher looks for a bound controller that is not there (unplugged, or not yet at the start).
#[cfg(windows)]
const LOOK_AGAIN: Duration = Duration::from_secs(2);

/// A game controller as the settings page lists it; `id` is what a binding keeps.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub id: String,
    pub name: String,
    pub buttons: u32,
}

/// A microphone as the settings' picker shows it; the id is what the settings keep.
#[derive(Debug, Clone, Serialize)]
pub struct Microphone {
    pub id: String,
    pub name: String,
}

/// What the watcher listens for: the bound button and the microphone to open while it is held.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub button: Button,
    /// The input device's id; None for the PC's default microphone
    pub microphone: Option<String>,
}

/// The first input that went down between two looks at the same inputs. An input that was down at the first look
/// counts only once it was released and pressed again.
pub fn went_down(before: &[bool], now: &[bool]) -> Option<usize> {
    before.iter().zip(now).position(|(before, now)| !before && *now)
}

/// The keys a binding may take: not the mouse buttons, not Escape (it cancels the capture) and not the generic
/// Shift, Ctrl and Alt (their left and right keys are taken instead).
pub fn bindable_key(code: usize) -> bool {
    !matches!(code, 0x00..=0x07 | 0x10..=0x12 | 0x1B | 0xFF) && code < 0x100
}

/// A recording while the button is held: the device's frames mixed down to mono, up to `MAX_SECONDS`.
pub struct Take {
    rate: u32,
    channels: usize,
    samples: Vec<f32>,
}

impl Take {
    pub fn new(rate: u32, channels: u16) -> Self {
        Self { rate, channels: usize::from(channels.max(1)), samples: Vec::with_capacity(rate as usize * 5) }
    }

    fn cap(&self) -> usize {
        (MAX_SECONDS * self.rate as f32) as usize
    }

    /// Adds interleaved frames; what goes beyond the cap is dropped.
    pub fn push(&mut self, interleaved: &[f32]) {
        let room = self.cap().saturating_sub(self.samples.len());
        let channels = self.channels;
        self.samples.extend(interleaved.chunks_exact(channels).take(room).map(|frame| frame.iter().sum::<f32>() / channels as f32));
    }

    pub fn full(&self) -> bool {
        self.samples.len() >= self.cap()
    }

    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.rate as f32
    }

    /// The samples and their rate when the take is long enough to be a question.
    pub fn question(self) -> Option<(Vec<f32>, u32)> {
        (self.seconds() >= MIN_SECONDS).then_some((self.samples, self.rate))
    }
}

#[cfg(windows)]
mod input {
    use std::ffi::c_void;
    use std::mem::size_of;

    use windows::core::{Interface, BOOL, GUID};
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::{HINSTANCE, HWND};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyNameTextW, MapVirtualKeyW, MAPVK_VK_TO_VSC_EX};

    use super::Controller;

    /// Not in the metadata the windows crate is made from: an object of the data format a device may lack.
    const DIDFT_OPTIONAL: u32 = 0x8000_0000;
    /// The axes the state reads, in the order of `State::axes`: X, Y, Z, rotation X, Y, Z, two sliders.
    static AXES: [GUID; 8] = [GUID_XAxis, GUID_YAxis, GUID_ZAxis, GUID_RxAxis, GUID_RyAxis, GUID_RzAxis, GUID_Slider, GUID_Slider];
    static POV: GUID = GUID_POV;

    /// What `GetDeviceState` fills: the layout of the data format below (DirectInput's own `c_dfDIJoystick2` is a
    /// data symbol of dinput8.lib, which the windows crate does not offer).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Raw {
        axes: [i32; 8],
        povs: [u32; 4],
        buttons: [u8; 128],
    }

    /// A controller's state at one look.
    #[derive(Debug, Clone, Copy)]
    pub struct State {
        pub axes: [i32; 8],
        /// Hat switches in hundredths of a degree, 0xFFFFFFFF when centred
        pub povs: [u32; 4],
        pub buttons: [bool; 128],
    }

    fn guid(guid: &GUID) -> String {
        format!("{:?}", guid)
    }

    /// A controller's id: its product (vendor and product id) and its instance on this PC. A binding finds the
    /// same instance, else the same product (the wheel in another USB port).
    fn id(instance: &DIDEVICEINSTANCEW) -> String {
        format!("{}/{}", guid(&instance.guidProduct), guid(&instance.guidInstance))
    }

    fn text(wide: &[u16]) -> String {
        String::from_utf16_lossy(&wide[..wide.iter().position(|c| *c == 0).unwrap_or(wide.len())])
    }

    fn name(instance: &DIDEVICEINSTANCEW) -> String {
        let name = text(&instance.tszInstanceName);
        if name.trim().is_empty() { text(&instance.tszProductName) } else { name }
    }

    /// DirectInput for this process.
    pub fn direct_input() -> Result<IDirectInput8W, String> {
        unsafe {
            let module = GetModuleHandleW(None).map_err(|error| format!("directinput: {error}"))?;
            let mut out: *mut c_void = std::ptr::null_mut();
            DirectInput8Create(HINSTANCE::from(module), DIRECTINPUT_VERSION, &IDirectInput8W::IID, &mut out, None).map_err(|error| format!("directinput: {error}"))?;
            Ok(IDirectInput8W::from_raw(out))
        }
    }

    unsafe extern "system" fn found(instance: *mut DIDEVICEINSTANCEW, list: *mut c_void) -> BOOL {
        unsafe { (*list.cast::<Vec<DIDEVICEINSTANCEW>>()).push(*instance) };
        BOOL(DIENUM_CONTINUE as i32)
    }

    /// The game controllers attached now.
    pub fn attached(input: &IDirectInput8W) -> Vec<DIDEVICEINSTANCEW> {
        let mut list: Vec<DIDEVICEINSTANCEW> = Vec::new();
        let _ = unsafe { input.EnumDevices(DI8DEVCLASS_GAMECTRL, Some(found), (&mut list as *mut Vec<DIDEVICEINSTANCEW>).cast(), DIEDFL_ATTACHEDONLY) };
        list
    }

    /// The attached controller a binding's id names: the same instance, else the same product.
    pub fn find(input: &IDirectInput8W, wanted: &str) -> Option<DIDEVICEINSTANCEW> {
        let list = attached(input);
        let product = wanted.split('/').next().unwrap_or_default();
        list.iter().find(|instance| id(instance) == wanted).or_else(|| list.iter().find(|instance| guid(&instance.guidProduct) == product)).copied()
    }

    /// An open controller, acquired nonexclusive in the background: the game may hold it exclusively for its force
    /// feedback meanwhile.
    pub struct Pad {
        pub id: String,
        pub name: String,
        pub buttons: u32,
        device: IDirectInputDevice8W,
    }

    impl Pad {
        pub fn open(input: &IDirectInput8W, instance: &DIDEVICEINSTANCEW) -> Result<Pad, String> {
            let fail = |step: &str, error: windows::core::Error| format!("{step}: {error}");
            unsafe {
                let mut device = None;
                input.CreateDevice(&instance.guidInstance, &mut device, None).map_err(|error| fail("create", error))?;
                let device = device.ok_or("create: no device")?;
                let flags = DIDFT_OPTIONAL | DIDFT_ANYINSTANCE;
                let mut objects: Vec<DIOBJECTDATAFORMAT> = Vec::with_capacity(140);
                for (index, axis) in AXES.iter().enumerate() {
                    objects.push(DIOBJECTDATAFORMAT { pguid: axis, dwOfs: (index * 4) as u32, dwType: DIDFT_AXIS | flags, dwFlags: DIDOI_ASPECTPOSITION });
                }
                for index in 0..4 {
                    objects.push(DIOBJECTDATAFORMAT { pguid: &POV, dwOfs: (32 + index * 4) as u32, dwType: DIDFT_POV | flags, dwFlags: 0 });
                }
                for index in 0..128 {
                    objects.push(DIOBJECTDATAFORMAT { pguid: std::ptr::null(), dwOfs: (48 + index) as u32, dwType: DIDFT_BUTTON | flags, dwFlags: 0 });
                }
                let mut format = DIDATAFORMAT {
                    dwSize: size_of::<DIDATAFORMAT>() as u32,
                    dwObjSize: size_of::<DIOBJECTDATAFORMAT>() as u32,
                    dwFlags: DIDF_ABSAXIS,
                    dwDataSize: size_of::<Raw>() as u32,
                    dwNumObjs: objects.len() as u32,
                    rgodf: objects.as_mut_ptr(),
                };
                device.SetDataFormat(&mut format).map_err(|error| fail("format", error))?;
                // background and nonexclusive: no window of ours needs the focus, and the game keeps its own access
                device.SetCooperativeLevel(HWND::default(), DISCL_BACKGROUND | DISCL_NONEXCLUSIVE).map_err(|error| fail("cooperative-level", error))?;
                let mut caps = DIDEVCAPS { dwSize: size_of::<DIDEVCAPS>() as u32, ..Default::default() };
                device.GetCapabilities(&mut caps).map_err(|error| fail("capabilities", error))?;
                device.Acquire().map_err(|error| fail("acquire", error))?;
                Ok(Pad { id: id(instance), name: name(instance), buttons: caps.dwButtons.min(128), device })
            }
        }

        /// Whether this is the controller a binding's id names (the instance, or the product when it came back
        /// in another port).
        pub fn matches(&self, wanted: &str) -> bool {
            self.id == wanted || self.id.split('/').next() == wanted.split('/').next()
        }

        /// Reads the state, acquiring the device again when the input was lost; an error means the controller is gone.
        pub fn read(&self) -> Result<State, String> {
            let mut raw = Raw { axes: [0; 8], povs: [0; 4], buttons: [0; 128] };
            let mut state = || unsafe {
                // devices that need no polling answer "no effect", which is fine
                let _ = self.device.Poll();
                self.device.GetDeviceState(size_of::<Raw>() as u32, (&mut raw as *mut Raw).cast())
            };
            let mut result = state();
            if let Err(error) = &result {
                if error.code() == DIERR_INPUTLOST || error.code() == DIERR_NOTACQUIRED {
                    unsafe { self.device.Acquire() }.map_err(|error| format!("acquire: {error}"))?;
                    result = state();
                }
            }
            result.map_err(|error| format!("state: {error}"))?;
            let mut buttons = [false; 128];
            for (button, value) in buttons.iter_mut().zip(raw.buttons) {
                *button = value & 0x80 != 0;
            }
            Ok(State { axes: raw.axes, povs: raw.povs, buttons })
        }
    }

    impl Drop for Pad {
        fn drop(&mut self) {
            let _ = unsafe { self.device.Unacquire() };
        }
    }

    /// The controllers attached now, for the settings page.
    pub fn controllers() -> Vec<Controller> {
        let Ok(input) = direct_input() else { return Vec::new() };
        attached(&input).iter().filter_map(|instance| Pad::open(&input, instance).ok()).map(|pad| Controller { id: pad.id.clone(), name: pad.name.clone(), buttons: pad.buttons }).collect()
    }

    /// Whether a key is held now, whichever window has the focus.
    pub fn key_down(code: u32) -> bool {
        (unsafe { GetAsyncKeyState(code as i32) } as u16) & 0x8000 != 0
    }

    /// Every key's state now, by virtual-key code.
    pub fn keys() -> [bool; 256] {
        let mut keys = [false; 256];
        for (code, key) in keys.iter_mut().enumerate() {
            *key = super::bindable_key(code) && key_down(code as u32);
        }
        keys
    }

    /// How the keyboard names a key (in the language of Windows), else its code.
    pub fn key_name(code: u32) -> String {
        let scan = unsafe { MapVirtualKeyW(code, MAPVK_VK_TO_VSC_EX) };
        let extended = if scan >> 8 == 0xE0 { 1 << 24 } else { 0 };
        let mut name = [0u16; 64];
        let length = unsafe { GetKeyNameTextW((((scan & 0xFF) << 16) | extended) as i32, &mut name) };
        if length > 0 { String::from_utf16_lossy(&name[..length as usize]) } else { format!("Key {code:#04X}") }
    }
}

#[cfg(windows)]
mod microphone {
    use std::str::FromStr;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use kira::backend::cpal::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use kira::backend::cpal::cpal::{self, DeviceId, FromSample, SampleFormat, SizedSample};

    use super::{Microphone, Take};

    pub fn inputs() -> Vec<Microphone> {
        let Ok(devices) = cpal::default_host().input_devices() else { return Vec::new() };
        devices.filter_map(|device| Some(Microphone { id: device.id().ok()?.to_string(), name: device.description().ok()?.name().to_string() })).collect()
    }

    /// The microphone while the button is held. Dropping it closes the device.
    pub struct Recorder {
        _stream: cpal::Stream,
        take: Arc<Mutex<Take>>,
        opened: Instant,
        first: Arc<OnceLock<Instant>>,
        pub rate: u32,
        pub channels: u16,
    }

    fn convert<T: SizedSample>(data: &cpal::Data, mono: &mut Vec<f32>) -> bool
    where
        f32: FromSample<T>,
    {
        let Some(samples) = data.as_slice::<T>() else { return false };
        mono.clear();
        mono.extend(samples.iter().map(|sample| sample.to_sample::<f32>()));
        true
    }

    impl Recorder {
        /// Opens `device` (gone or None: the PC's default microphone) and records until it is finished.
        pub fn start(device: Option<&str>) -> Result<Recorder, String> {
            let opened = Instant::now();
            let host = cpal::default_host();
            let chosen = device.and_then(|id| DeviceId::from_str(id).ok()).and_then(|id| host.device_by_id(&id));
            let device = chosen.or_else(|| host.default_input_device()).ok_or("no-microphone")?;
            let config = device.default_input_config().map_err(|error| format!("no-microphone: {error}"))?;
            let format = config.sample_format();
            if !matches!(format, SampleFormat::F32 | SampleFormat::F64 | SampleFormat::I16 | SampleFormat::I32 | SampleFormat::I8 | SampleFormat::U8 | SampleFormat::U16) {
                return Err(format!("unsupported-format: {format}"));
            }
            let (rate, channels) = (config.sample_rate(), config.channels());
            let take = Arc::new(Mutex::new(Take::new(rate, channels)));
            let first = Arc::new(OnceLock::new());
            let (into, seen) = (take.clone(), first.clone());
            let mut mono = Vec::new();
            let stream = device
                .build_input_stream_raw(
                    config.config(),
                    format,
                    move |data: &cpal::Data, _: &cpal::InputCallbackInfo| {
                        seen.get_or_init(Instant::now);
                        let mut take = into.lock().unwrap();
                        if let Some(samples) = data.as_slice::<f32>() {
                            take.push(samples);
                            return;
                        }
                        let converted = match data.sample_format() {
                            SampleFormat::F64 => convert::<f64>(data, &mut mono),
                            SampleFormat::I16 => convert::<i16>(data, &mut mono),
                            SampleFormat::I32 => convert::<i32>(data, &mut mono),
                            SampleFormat::I8 => convert::<i8>(data, &mut mono),
                            SampleFormat::U8 => convert::<u8>(data, &mut mono),
                            SampleFormat::U16 => convert::<u16>(data, &mut mono),
                            _ => false,
                        };
                        if converted {
                            take.push(&mono);
                        }
                    },
                    // a device that goes away stops delivering: the take ends where it was
                    |_error| {},
                    None,
                )
                .map_err(|error| format!("no-microphone: {error}"))?;
            stream.play().map_err(|error| format!("no-microphone: {error}"))?;
            Ok(Recorder { _stream: stream, take, opened, first, rate, channels })
        }

        /// How long it took from `start` until the device delivered its first samples; None while it has not.
        pub fn live_after(&self) -> Option<Duration> {
            self.first.get().map(|first| first.duration_since(self.opened))
        }

        pub fn full(&self) -> bool {
            self.take.lock().unwrap().full()
        }

        /// Closes the microphone and answers what it recorded.
        pub fn finish(self) -> Take {
            let Recorder { _stream, take, rate, channels, .. } = self;
            drop(_stream);
            let mut take = take.lock().unwrap();
            std::mem::replace(&mut *take, Take::new(rate, channels))
        }
    }
}

#[cfg(windows)]
pub use input::{key_down, key_name, keys, Pad, State};
#[cfg(windows)]
pub use microphone::Recorder;

/// DirectInput for this process and its attached controllers, for a caller that reads every controller (the
/// probe, the capture).
#[cfg(windows)]
pub fn open_all() -> Result<Vec<Pad>, String> {
    let input = input::direct_input()?;
    let pads = input::attached(&input).iter().filter_map(|instance| Pad::open(&input, instance).ok()).collect();
    Ok(pads)
}

/// The game controllers attached now.
pub fn controllers() -> Vec<Controller> {
    #[cfg(windows)]
    return input::controllers();
    #[cfg(not(windows))]
    Vec::new()
}

/// The microphones of this PC.
pub fn inputs() -> Vec<Microphone> {
    #[cfg(windows)]
    return microphone::inputs();
    #[cfg(not(windows))]
    Vec::new()
}

/// While a capture runs the watcher takes no button as pressed: the driver presses buttons to choose one.
#[cfg(windows)]
static CAPTURING: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
struct Capturing;

#[cfg(windows)]
impl Drop for Capturing {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::SeqCst);
    }
}

/// Waits for the next button going down on any controller or the next key going down on the keyboard and answers
/// it as a binding. What is held when the capture starts counts only once it was released and pressed again; mouse
/// buttons do not count; Escape cancels (error `cancelled`); nothing within `timeout` is error `timeout`.
pub fn capture(timeout: Duration) -> Result<Button, String> {
    #[cfg(windows)]
    {
        use std::time::Instant;
        CAPTURING.store(true, Ordering::SeqCst);
        let _capturing = Capturing;
        let pads = open_all()?;
        let mut keys = input::keys();
        let mut escape = key_down(0x1B);
        let mut states: Vec<Option<[bool; 128]>> = pads.iter().map(|pad| pad.read().ok().map(|state| state.buttons)).collect();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            std::thread::sleep(POLL);
            let escape_now = key_down(0x1B);
            if escape_now && !escape {
                return Err("cancelled".to_string());
            }
            escape = escape_now;
            let now = input::keys();
            if let Some(code) = went_down(&keys, &now) {
                return Ok(Button::Key { code: code as u32, name: key_name(code as u32) });
            }
            keys = now;
            for (pad, before) in pads.iter().zip(states.iter_mut()) {
                let Ok(state) = pad.read() else { continue };
                if let Some(button) = before.and_then(|before| went_down(&before[..pad.buttons as usize], &state.buttons[..pad.buttons as usize])) {
                    return Ok(Button::Controller { device: pad.id.clone(), device_name: pad.name.clone(), button: button as u32 });
                }
                *before = Some(state.buttons);
            }
        }
        Err("timeout".to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = timeout;
        Err("unsupported".to_string())
    }
}

/// Watches the bound button every `POLL` in a thread of its own, for as long as the app runs. `binding` is asked at
/// every look (None: the coach's microphone is off, nothing is read). The microphone opens when the button goes
/// down and closes when it goes up or after `MAX_SECONDS`. `on_live` is called once per press when the microphone
/// delivers its first samples (40 to 300 ms after the press, measured 2026-11-09): the moment to tell the driver
/// to speak. `on_question` gets every recording of at least `MIN_SECONDS` (mono samples, their rate). A bound
/// controller that is not there is looked for every 2 s, so one plugged in later or replugged is found again. A
/// button held when it was bound counts once it was released.
pub fn watch(binding: impl FnMut() -> Option<Binding> + Send + 'static, on_live: impl FnMut() + Send + 'static, on_question: impl FnMut(Vec<f32>, u32) + Send + 'static) {
    #[cfg(windows)]
    std::thread::spawn(move || watch_loop(binding, on_live, on_question));
    #[cfg(not(windows))]
    let _ = (binding, on_live, on_question);
}

#[cfg(windows)]
fn watch_loop(mut binding: impl FnMut() -> Option<Binding>, mut on_live: impl FnMut(), mut on_question: impl FnMut(Vec<f32>, u32)) {
    use crate::log;
    use std::time::Instant;
    let input = match input::direct_input() {
        Ok(input) => Some(input),
        Err(error) => {
            log(&format!("talk: no controllers: {error}"));
            None
        }
    };
    let mut pad: Option<Pad> = None;
    let mut looked: Option<Instant> = None;
    let mut bound: Option<Button> = None;
    let mut was_down = false;
    let mut recorder: Option<Recorder> = None;
    let mut told_live = false;
    loop {
        std::thread::sleep(POLL);
        let Some(wanted) = binding() else {
            pad = None;
            bound = None;
            recorder = None;
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        let down = match &wanted.button {
            Button::Key { code, .. } => key_down(*code),
            Button::Controller { device, button, .. } => {
                if pad.as_ref().is_some_and(|pad| !pad.matches(device)) {
                    pad = None;
                }
                if pad.is_none() && looked.is_none_or(|looked| looked.elapsed() >= LOOK_AGAIN) {
                    looked = Some(Instant::now());
                    if let Some(input) = &input {
                        pad = input::find(input, device).and_then(|instance| match Pad::open(input, &instance) {
                            Ok(pad) => {
                                log(&format!("talk: watching button {} of {}", button + 1, pad.name));
                                Some(pad)
                            }
                            Err(error) => {
                                log(&format!("talk: the bound controller does not open: {error}"));
                                None
                            }
                        });
                    }
                }
                match pad.as_ref().map(Pad::read) {
                    Some(Ok(state)) => state.buttons[*button as usize],
                    Some(Err(error)) => {
                        log(&format!("talk: the bound controller is gone: {error}"));
                        pad = None;
                        false
                    }
                    None => false,
                }
            }
        } && !CAPTURING.load(Ordering::SeqCst);
        if bound.as_ref() != Some(&wanted.button) {
            // a new binding: a button held now counts once it was released
            bound = Some(wanted.button.clone());
            was_down = down;
            recorder = None;
            continue;
        }
        if down && !was_down {
            told_live = false;
            recorder = match Recorder::start(wanted.microphone.as_deref()) {
                Ok(recorder) => Some(recorder),
                Err(error) => {
                    log(&format!("talk: the microphone does not open: {error}"));
                    None
                }
            };
        }
        if let Some(open) = &recorder {
            if !told_live {
                if let Some(after) = open.live_after() {
                    told_live = true;
                    log(&format!("talk: the microphone delivers {} ms after the button went down", after.as_millis()));
                    on_live();
                }
            }
        }
        let ended = (!down && was_down) || recorder.as_ref().is_some_and(Recorder::full);
        if ended {
            if let Some(take) = recorder.take().map(Recorder::finish) {
                let seconds = take.seconds();
                match take.question() {
                    Some((samples, rate)) => {
                        log(&format!("talk: a question of {seconds:.1} s"));
                        on_question(samples, rate);
                    }
                    None => log(&format!("talk: {seconds:.2} s is too short for a question")),
                }
            }
        }
        was_down = down;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_counts_when_it_goes_down_and_one_held_from_the_start_only_after_it_was_released() {
        let up = [false; 4];
        assert_eq!(went_down(&up, &up), None);
        assert_eq!(went_down(&up, &[false, false, true, false]), Some(2));
        assert_eq!(went_down(&[false, false, true, false], &[false, false, true, false]), None, "held is not pressed again");
        // button 1 is held when the capture starts: holding it is nothing, releasing it is nothing, pressing it again counts
        let start = [false, true, false, false];
        assert_eq!(went_down(&start, &start), None);
        assert_eq!(went_down(&start, &up), None);
        assert_eq!(went_down(&up, &start), Some(1));
        // of two going down at the same look, the first counts
        assert_eq!(went_down(&up, &[true, false, false, true]), Some(0));
    }

    #[test]
    fn mouse_buttons_escape_and_the_generic_modifiers_are_not_bound() {
        for code in [0x01, 0x02, 0x04, 0x05, 0x06, 0x10, 0x11, 0x12, 0x1B] {
            assert!(!bindable_key(code), "{code:#x}");
        }
        for code in [0x08, 0x20, 0x41, 0x7C, 0xA0, 0xA3, 0x91] {
            assert!(bindable_key(code), "{code:#x}");
        }
    }

    #[test]
    fn a_take_is_mixed_down_to_mono_and_stops_at_the_cap() {
        let mut take = Take::new(10, 2);
        take.push(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0]);
        assert_eq!(take.samples, vec![0.5, 0.5, 0.0]);
        // 20 s at 10 Hz is 200 samples: what comes later is dropped
        take.push(&vec![0.2; 2 * 300]);
        assert_eq!(take.samples.len(), 200);
        assert!(take.full());
        assert_eq!(take.seconds(), MAX_SECONDS);
        let mut mono = Take::new(48_000, 1);
        mono.push(&[0.25; 480]);
        assert_eq!(mono.samples.len(), 480);
        assert!(!mono.full());
    }

    #[test]
    fn a_press_shorter_than_a_third_of_a_second_is_no_question() {
        let mut short = Take::new(1000, 1);
        short.push(&[0.1; 299]);
        assert!(short.question().is_none());
        let mut long = Take::new(1000, 4);
        long.push(&[0.1; 300 * 4]);
        let (samples, rate) = long.question().unwrap();
        assert_eq!((samples.len(), rate), (300, 1000));
    }

    /// Needs a game controller on this PC: `cargo test -p dd-desktop finds_and_reads -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn finds_and_reads_the_attached_controllers_by_instance_and_by_product() {
        let input = input::direct_input().unwrap();
        let attached = input::attached(&input);
        assert!(!attached.is_empty(), "no controller attached");
        for pad in open_all().unwrap() {
            let product = pad.id.split('/').next().unwrap();
            let by_id = Pad::open(&input, &input::find(&input, &pad.id).unwrap()).unwrap();
            let by_product = Pad::open(&input, &input::find(&input, &format!("{product}/00000000-0000-0000-0000-000000000000")).unwrap()).unwrap();
            assert_eq!((by_id.id.as_str(), by_product.id.as_str()), (pad.id.as_str(), pad.id.as_str()));
            assert!(pad.matches(&format!("{product}/another")) && !pad.matches("another/x"));
            for _ in 0..20 {
                let state = pad.read().unwrap();
                assert!(state.buttons[pad.buttons as usize..].iter().all(|down| !down));
                std::thread::sleep(POLL);
            }
            println!("{} ({} buttons): read 20 times", pad.name, pad.buttons);
        }
    }
}
