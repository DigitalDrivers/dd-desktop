//! Checks the coach's push-to-talk on this PC without the app: lists the game controllers and microphones, then
//! prints every controller button that goes down or up, which window has the focus and the first axes, so a driver
//! sees whether the app reads his wheel while the game is in the foreground.
//!
//! ```text
//! cargo build -p dd-desktop --example ptt-probe
//! target\debug\examples\ptt-probe.exe [--record] [--keep <file.wav>] [--bind] [--key <code>] [--seconds <n>]
//! ```
//!
//! `--record` opens the PC's default microphone (or `--mic <id>`) while any controller button or the key (`--key`,
//! a virtual-key code, default the space bar 0x20) is held and prints, per recording, the time from the press to
//! the first samples, its length, peak and RMS; nothing is written unless `--keep <file.wav>` names a file.
//! `--bind` asks for a button first (with `--press`: takes that key) and then runs the app's own watcher on it (`talk::capture`, `talk::watch`).
//! `--seconds` ends after that many seconds; `--press <code>:<seconds>[:<times>]` presses a key with `SendInput` after one
//! second and holds it (`:<times>` repeats it), for automated checks.

#[cfg(windows)]
fn main() {
    probe::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ptt-probe only runs on Windows");
}

#[cfg(windows)]
mod probe {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use dd_desktop_lib::talk::{self, Binding, Pad, Recorder, Take};
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};

    struct Args {
        record: bool,
        keep: Option<String>,
        bind: bool,
        key: u32,
        mic: Option<String>,
        seconds: Option<f32>,
        press: Option<(u32, f32, u32)>,
    }

    fn code(text: &str) -> u32 {
        let text = text.trim();
        text.strip_prefix("0x").map_or_else(|| text.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok()).unwrap_or_else(|| panic!("not a key code: {text}"))
    }

    fn args() -> Args {
        let mut args = Args { record: false, keep: None, bind: false, key: 0x20, mic: None, seconds: None, press: None };
        let mut list = std::env::args().skip(1);
        while let Some(arg) = list.next() {
            let mut value = || list.next().unwrap_or_else(|| panic!("{arg} needs a value"));
            match arg.as_str() {
                "--record" => args.record = true,
                "--keep" => args.keep = Some(value()),
                "--bind" => args.bind = true,
                "--key" => args.key = code(&value()),
                "--mic" => args.mic = Some(value()),
                "--seconds" => args.seconds = Some(value().parse().expect("--seconds <n>")),
                "--press" => {
                    let value = value();
                    let parts: Vec<&str> = value.split(':').collect();
                    let usage = "--press <code>:<seconds>[:<times>]";
                    let times = parts.get(2).map_or(1, |times| times.parse().expect(usage));
                    args.press = Some((code(parts[0]), parts.get(1).expect(usage).parse().expect(usage), times));
                }
                other => panic!("unknown argument {other}"),
            }
        }
        args
    }

    /// The window that has the focus, and whether it is one of this process.
    fn foreground() -> (String, bool) {
        unsafe {
            let window = GetForegroundWindow();
            let mut title = [0u16; 120];
            let length = GetWindowTextW(window, &mut title);
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            (String::from_utf16_lossy(&title[..length.max(0) as usize]), pid == std::process::id())
        }
    }

    fn send_key(code: u32, up: bool) {
        let flags = if up { KEYEVENTF_KEYUP } else { Default::default() };
        let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(code as u16), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
        let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        println!("[SendInput] Taste {code:#04X} {} ({sent} Ereignis gesendet)", if up { "los" } else { "gedrückt" });
    }

    /// Peak and RMS of samples, in dBFS.
    fn levels(samples: &[f32]) -> (f32, f32) {
        let peak = samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len().max(1) as f32).sqrt();
        let db = |value: f32| 20.0 * value.max(1e-9).log10();
        (db(peak), db(rms))
    }

    /// A mono 16-bit WAV, only when the driver asks for one with `--keep`.
    fn write_wav(path: &str, samples: &[f32], rate: u32) {
        let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
        let data = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap_or_else(|error| panic!("{path}: {error}"));
        println!("  gespeichert: {path}");
    }

    fn report(take: Take, pressed: Instant, live_after: Option<Duration>, rate: u32, channels: u16, keep: Option<&str>) {
        let seconds = take.seconds();
        let delay = live_after.map_or("keine Samples".to_string(), |after| format!("{} ms", after.as_millis()));
        println!("Aufnahme: {seconds:.2} s gehalten {:.2} s, erste Samples nach {delay}, {rate} Hz, {channels} Kanal/Kanäle", pressed.elapsed().as_secs_f32());
        match take.question() {
            Some((samples, rate)) => {
                let (peak, rms) = levels(&samples);
                println!("  Pegel: Spitze {peak:.1} dBFS, RMS {rms:.1} dBFS ({} Samples mono)", samples.len());
                if let Some(path) = keep {
                    write_wav(path, &samples, rate);
                }
            }
            None => println!("  kürzer als {} s: zählt nicht als Frage", talk::MIN_SECONDS),
        }
    }

    pub fn run() {
        let args = args();
        let started = Instant::now();
        println!("Lenkräder und Controller:");
        let controllers = talk::controllers();
        for controller in &controllers {
            println!("  {}  ({} Knöpfe)  id {}", controller.name, controller.buttons, controller.id);
        }
        if controllers.is_empty() {
            println!("  keine gefunden");
        }
        println!("Mikrofone:");
        let default = {
            use kira::backend::cpal::cpal::traits::{DeviceTrait, HostTrait};
            kira::backend::cpal::cpal::default_host().default_input_device().and_then(|device| device.id().ok()).map(|id| id.to_string())
        };
        for microphone in talk::inputs() {
            let marker = if default.as_deref() == Some(microphone.id.as_str()) { "  (Standard)" } else { "" };
            println!("  {}{marker}  id {}", microphone.name, microphone.id);
        }
        if let Some((key, seconds, times)) = args.press {
            std::thread::spawn(move || {
                for _ in 0..times {
                    std::thread::sleep(Duration::from_secs(1));
                    send_key(key, false);
                    std::thread::sleep(Duration::from_secs_f32(seconds));
                    send_key(key, true);
                }
            });
        }
        let ends = |started: Instant| args.seconds.is_some_and(|seconds| started.elapsed().as_secs_f32() >= seconds);
        if args.bind {
            bind(&args, started, ends);
        } else {
            watch_all(&args, started, ends);
        }
    }

    /// Prints what every controller does, and with `--record` records while any button is held.
    fn watch_all(args: &Args, started: Instant, ends: impl Fn(Instant) -> bool) {
        let pads: Vec<Pad> = talk::open_all().unwrap_or_else(|error| panic!("{error}"));
        println!("Drück Knöpfe am Lenkrad (Strg+C beendet). Der Probe liest auch, wenn das Spiel vorne ist.");
        if args.record {
            println!("Halte einen Knopf oder die Taste {} gedrückt, um aufzunehmen.", talk::key_name(args.key));
        }
        let mut states: Vec<Option<talk::State>> = vec![None; pads.len()];
        let mut errors = 0u32;
        let mut reads = 0u64;
        let mut key_was = talk::key_down(args.key);
        let mut recording: Option<(Recorder, Instant)> = None;
        let mut told = Instant::now();
        while !ends(started) {
            std::thread::sleep(talk::POLL);
            let mut any = false;
            for (pad, before) in pads.iter().zip(states.iter_mut()) {
                let state = match pad.read() {
                    Ok(state) => state,
                    Err(error) => {
                        errors += 1;
                        if errors <= 5 {
                            println!("{}: Lesefehler {error}", pad.name);
                        }
                        continue;
                    }
                };
                reads += 1;
                if let Some(before) = before {
                    for button in 0..pad.buttons as usize {
                        if state.buttons[button] != before.buttons[button] {
                            println!("{}: Knopf {} {}", pad.name, button + 1, if state.buttons[button] { "gedrückt" } else { "losgelassen" });
                        }
                    }
                    for hat in 0..4 {
                        if state.povs[hat] != before.povs[hat] {
                            println!("{}: Steuerkreuz {} auf {}", pad.name, hat + 1, if state.povs[hat] == u32::MAX { "Mitte".to_string() } else { format!("{}°", state.povs[hat] / 100) });
                        }
                    }
                }
                any |= state.buttons[..pad.buttons as usize].iter().any(|down| *down);
                *before = Some(state);
            }
            let key = talk::key_down(args.key);
            if key != key_was {
                println!("Taste {}: {}", talk::key_name(args.key), if key { "gedrückt" } else { "losgelassen" });
                key_was = key;
            }
            any |= key;
            if args.record {
                match (&recording, any) {
                    (None, true) => match Recorder::start(args.mic.as_deref()) {
                        Ok(recorder) => recording = Some((recorder, Instant::now())),
                        Err(error) => println!("Mikrofon öffnet nicht: {error}"),
                    },
                    (Some((recorder, _)), held) if !held || recorder.full() => {
                        let (recorder, pressed) = recording.take().expect("matched");
                        let (live_after, rate, channels) = (recorder.live_after(), recorder.rate, recorder.channels);
                        report(recorder.finish(), pressed, live_after, rate, channels, args.keep.as_deref());
                    }
                    _ => {}
                }
            }
            if told.elapsed() >= Duration::from_secs(2) {
                told = Instant::now();
                let (title, ours) = foreground();
                let axes: Vec<String> = pads.iter().zip(&states).filter_map(|(pad, state)| state.map(|state| format!("{}: X {} Y {} Z {} Rz {}", pad.name, state.axes[0], state.axes[1], state.axes[2], state.axes[5]))).collect();
                println!("[{:4.0} s] vorne: \"{title}\" ({}) | {} | {reads} Lesungen, {errors} Fehler", started.elapsed().as_secs_f32(), if ours { "unser Fenster" } else { "fremdes Fenster" }, axes.join(" | "));
            }
        }
        println!("Ende: {reads} Lesungen, {errors} Fehler");
    }

    /// Asks for a button as the settings page does and runs the app's watcher on it.
    fn bind(args: &Args, started: Instant, ends: impl Fn(Instant) -> bool) {
        // an automated check binds the key it presses, without a capture (keys typed meanwhile would win it)
        let button = if let Some((code, _, _)) = args.press {
            dd_core::radio::settings::Button::Key { code, name: talk::key_name(code) }
        } else {
            println!("Drück jetzt den Knopf (oder die Taste), mit dem du den Coach fragen willst. Esc bricht ab.");
            match talk::capture(Duration::from_secs(20)) {
                Ok(button) => button,
                Err(error) => {
                    println!("Kein Knopf: {error}");
                    return;
                }
            }
        };
        println!("Gewählt: {button:?}");
        println!("Halte ihn gedrückt und sprich; loslassen beendet die Aufnahme.");
        let (sender, questions) = mpsc::channel();
        let binding = Binding { button, microphone: args.mic.clone() };
        talk::watch(move || Some(binding.clone()), || println!("Mikrofon offen: jetzt sprechen"), move |samples, rate| {
            let _ = sender.send((samples, rate));
        });
        while !ends(started) {
            if let Ok((samples, rate)) = questions.recv_timeout(Duration::from_millis(200)) {
                let (peak, rms) = levels(&samples);
                println!("Frage: {:.2} s, {rate} Hz, Spitze {peak:.1} dBFS, RMS {rms:.1} dBFS", samples.len() as f32 / rate as f32);
                if let Some(path) = &args.keep {
                    write_wav(path, &samples, rate);
                }
            }
        }
    }
}
