//! What the team radio says, as text: the wordings of every call in German and English (`phrases.json`), the
//! sentences with numbers built at run time, and the name of the cached sound of a wording. The voice (dd-voice)
//! renders the fixed wordings once per voice and language; the names of other drivers are rendered when the
//! driver joins a server; sentences with numbers are not urgent and are spoken live.
//!
//! Numbers are always spelled out here. Supertonic reads digits itself, but a lap time as "8:12,4" came out about
//! 20 % longer than "acht zwölf vier" (duration predictor, 2026-11-09), so it does not say it the way a race
//! engineer does, and "21" came out shorter than "einundzwanzig".

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The version of `phrases.json`.
pub const VERSION: u32 = 1;
/// The model and render settings the cached sounds depend on (dd-voice checks that it says the same). A change here
/// gives every wording a new cache name, so the library renders itself again.
pub const MODEL: &str = "supertonic-3@724fb5abbf5502583fb520898d45929e62f02c0b/steps-8/speed-1.05/seed-1";
/// The languages every call has.
pub const LANGUAGES: [&str; 2] = ["de", "en"];
/// Where the driver's spoken name goes in a named call.
pub const NAME: &str = "{name}";

/// The calls without a name: what the spotter, the queue and the engineer emit, and the radio's own sentences.
const FIXED: [&str; 33] = [
    "car_left",
    "car_right",
    "three_wide",
    "three_wide_left",
    "three_wide_right",
    "still_there",
    "clear_left",
    "clear_right",
    "clear",
    "closing_fast",
    "close_behind",
    "slipstream_behind",
    "in_tow",
    "slow_car_ahead",
    "radio_check",
    "copy_moment",
    "coach_tank_empty",
    "coach_not_available",
    "coach_failed",
    "coach_not_understood",
    "fuel_low",
    "fuel_box_this_lap",
    "lap_invalid",
    "brakes_hot",
    "tyres_in_window",
    "limiter_on",
    "limiter_off",
    "pit_entry_speed",
    "pit_ten",
    "pit_five",
    "pit_go",
    "pit_exit_traffic",
    "pit_exit_clear",
];
/// The spotter's calls with another driver's name, rendered per driver.
const NAMED: [&str; 6] =
    ["name_left", "name_right", "name_closing_fast", "name_close_behind", "name_slipstream_behind", "name_in_tow"];
/// The two sentences the name preview speaks the driver's own name in.
const PREVIEW: [&str; 2] = ["preview_vocative", "preview_reference"];

#[derive(Deserialize)]
struct Library {
    phrases: BTreeMap<String, Wordings>,
}

#[derive(Deserialize)]
struct Wordings {
    de: Vec<String>,
    en: Vec<String>,
}

fn library() -> &'static Library {
    static LIBRARY: OnceLock<Library> = OnceLock::new();
    LIBRARY.get_or_init(|| serde_json::from_str(include_str!("phrases.json")).expect("phrases.json is valid"))
}

pub fn fixed_keys() -> &'static [&'static str] {
    &FIXED
}

pub fn named_keys() -> &'static [&'static str] {
    &NAMED
}

pub fn preview_keys() -> &'static [&'static str] {
    &PREVIEW
}

/// All wordings of a call in a language, with the `{name}` placeholder left in; empty for an unknown key or
/// language.
pub fn wordings(key: &str, lang: &str) -> &'static [String] {
    match (library().phrases.get(key), lang) {
        (Some(w), "de") => &w.de,
        (Some(w), "en") => &w.en,
        _ => &[],
    }
}

/// Wording number `pick` (wrapping round) of a call, with `name` in place of `{name}`. None for an unknown key or
/// language, and for a named call without a name; a name given to a call without a placeholder is ignored.
pub fn text(key: &str, lang: &str, name: Option<&str>, pick: usize) -> Option<String> {
    let all = wordings(key, lang);
    let wording = all.get(pick % all.len().max(1))?;
    if !wording.contains(NAME) {
        return Some(wording.clone());
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty())?;
    Some(wording.replace(NAME, name))
}

/// The file-system-safe name (no extension) of the cached sound of `text` spoken by `voice` in `lang`: voice and
/// language for a human looking at the folder, then the SHA-256 of model, voice, language and text.
pub fn cache_name(voice: &str, lang: &str, text: &str) -> String {
    let hash = Sha256::digest(format!("{MODEL}|{voice}|{lang}|{text}").as_bytes());
    let hex: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
    let safe = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).take(8).collect::<String>();
    format!("{}-{}-{hex}", safe(voice), safe(lang))
}

const DE_UNITS: [&str; 20] = [
    "null", "eins", "zwei", "drei", "vier", "fünf", "sechs", "sieben", "acht", "neun", "zehn", "elf", "zwölf",
    "dreizehn", "vierzehn", "fünfzehn", "sechzehn", "siebzehn", "achtzehn", "neunzehn",
];
const DE_TENS: [&str; 10] = ["", "", "zwanzig", "dreißig", "vierzig", "fünfzig", "sechzig", "siebzig", "achtzig", "neunzig"];
const EN_UNITS: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
];
const EN_TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// A number as German counts it ("eins", "einundzwanzig", "einhundertzwölf"); digits from 1,000 on.
pub(crate) fn de_number(n: u64) -> String {
    let prefix = |u: u64| if u == 1 { "ein".to_string() } else { DE_UNITS[u as usize].to_string() };
    match n {
        0..=19 => DE_UNITS[n as usize].to_string(),
        20..=99 if n.is_multiple_of(10) => DE_TENS[(n / 10) as usize].to_string(),
        20..=99 => format!("{}und{}", prefix(n % 10), DE_TENS[(n / 10) as usize]),
        100..=999 if n.is_multiple_of(100) => format!("{}hundert", prefix(n / 100)),
        100..=999 => format!("{}hundert{}", prefix(n / 100), de_number(n % 100)),
        _ => n.to_string(),
    }
}

/// A number in British English ("twenty-one", "one hundred and five"); digits from 1,000 on.
pub(crate) fn en_number(n: u64) -> String {
    match n {
        0..=19 => EN_UNITS[n as usize].to_string(),
        20..=99 if n.is_multiple_of(10) => EN_TENS[(n / 10) as usize].to_string(),
        20..=99 => format!("{}-{}", EN_TENS[(n / 10) as usize], EN_UNITS[(n % 10) as usize]),
        100..=999 if n.is_multiple_of(100) => format!("{} hundred", EN_UNITS[(n / 100) as usize]),
        100..=999 => format!("{} hundred and {}", EN_UNITS[(n / 100) as usize], en_number(n % 100)),
        _ => n.to_string(),
    }
}

fn capitalised(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// How far the fuel goes, rounded down to half a lap (the engineer never promises more than is in the tank):
/// "Sprit für noch vier Runden." / "Fuel for four more laps.", with "eine Runde", "anderthalb", "zweieinhalb",
/// "half a lap", "one and a half". Any language other than "de" is English.
pub fn fuel_laps(laps: f64, lang: &str) -> String {
    let halves = if laps.is_finite() && laps > 0.0 { (laps * 2.0).floor() as u64 } else { 0 };
    let (whole, half) = (halves / 2, halves % 2 == 1);
    if lang == "de" {
        match (whole, half) {
            (0, false) => "Sprit für weniger als eine halbe Runde.".to_string(),
            (0, true) => "Sprit für noch eine halbe Runde.".to_string(),
            (1, false) => "Sprit für noch eine Runde.".to_string(),
            (1, true) => "Sprit für noch anderthalb Runden.".to_string(),
            (n, false) => format!("Sprit für noch {} Runden.", de_number(n)),
            // A trailing counting "eins" becomes "ein": 101.5 is "einhunderteineinhalb".
            (n, true) => {
                let n = de_number(n);
                let stem = n.strip_suffix("eins").map(|s| format!("{s}ein")).unwrap_or(n);
                format!("Sprit für noch {stem}einhalb Runden.")
            }
        }
    } else {
        match (whole, half) {
            (0, false) => "Fuel for less than half a lap.".to_string(),
            (0, true) => "Fuel for half a lap.".to_string(),
            (1, false) => "Fuel for one more lap.".to_string(),
            (n, false) => format!("Fuel for {} more laps.", en_number(n)),
            (n, true) => format!("Fuel for {} and a half more laps.", en_number(n)),
        }
    }
}

/// A lap time as a race engineer says it, in tenths, cut (not rounded) like a timing screen: 8:12.4 is "acht
/// zwölf vier" / "eight twelve four", 1:05.3 "eins null fünf drei" / "one oh five three" (the zero stays, so it
/// cannot be heard as 1:53), 58.3 s "achtundfünfzig drei" / "fifty-eight three". No full stop, lower case.
fn lap_time_words(lap_ms: i64, lang: &str) -> String {
    let tenths = lap_ms.max(0) as u64 / 100;
    let (minutes, seconds, tenth) = (tenths / 600, tenths / 10 % 60, tenths % 10);
    let (number, oh, zero): (fn(u64) -> String, &str, &str) =
        if lang == "de" { (de_number, "null", "null") } else { (en_number, "oh", "zero") };
    let tenth = if tenth == 0 { zero.to_string() } else { number(tenth) };
    match (minutes, seconds) {
        (0, s) => format!("{} {tenth}", number(s)),
        (m, s) if s < 10 => format!("{} {oh} {} {tenth}", number(m), if s == 0 { oh.to_string() } else { number(s) }),
        (m, s) => format!("{} {} {tenth}", number(m), number(s)),
    }
}

/// A sentence of the coach's model with its numbers spelled out, so the voice says them as an engineer does: a lap
/// time "1:12,4" becomes "eins zwölf vier", a decimal "26,4" "sechsundzwanzig Komma vier" ("point" in English, where
/// the decimal sign is the point), a whole number up to 999 its word; "85%" gets "Prozent", "0,3 s" "Sekunden", a
/// sign "minus" or "plus". What is not such a number (thousands "1.000", a date "12.11.", a version "0.9.1", three
/// decimals, part of a word like "T13") stays whole as written.
pub fn spoken(text: &str, lang: &str) -> String {
    let german = lang == "de";
    let number: fn(u64) -> String = if german { de_number } else { en_number };
    let decimal_sign = if german { ',' } else { '.' };
    let chars: Vec<char> = text.chars().collect();
    let digit = |at: usize| chars.get(at).is_some_and(char::is_ascii_digit);
    let digits_from = |at: usize| chars[at.min(chars.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
    let value = |at: usize, len: usize| chars[at..at + len].iter().collect::<String>().parse::<u64>().unwrap_or(0);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // a sign in front of a number, not a hyphen inside a word
        if matches!(c, '-' | '+' | '\u{2212}') && digit(i + 1) && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(') {
            out.push_str(if c == '+' { "plus " } else { "minus " });
            i += 1;
            continue;
        }
        if !c.is_ascii_digit() {
            out.push(c);
            i += 1;
            continue;
        }
        // the whole token: digits, and digits joined to them by a point, a comma or a colon
        let whole = digits_from(i);
        let mut groups: Vec<(char, usize, usize)> = Vec::new();
        let mut end = i + whole;
        while matches!(chars.get(end), Some('.' | ',' | ':')) && digit(end + 1) {
            let len = digits_from(end + 1);
            groups.push((chars[end], end + 1, len));
            end += 1 + len;
        }
        let inside_word = i > 0 && chars[i - 1].is_alphanumeric();
        let n = value(i, whole.min(18));
        let words = match groups.as_slice() {
            _ if inside_word || whole > 3 => None,
            [] => Some(number(n)),
            // a lap time, with or without its tenths
            [(':', at, 2)] => {
                let zero = if german { "null" } else { "oh" };
                Some(match value(*at, 2) {
                    0 => format!("{} {zero} {zero}", number(n)),
                    s @ 1..=9 => format!("{} {zero} {}", number(n), number(s)),
                    s => format!("{} {}", number(n), number(s)),
                })
            }
            [(':', at, 2), ('.' | ',', tenths, 1..)] => {
                let tenth = chars[*tenths].to_digit(10).unwrap_or(0) as i64;
                Some(lap_time_words((n as i64 * 60 + value(*at, 2) as i64) * 1000 + tenth * 100, lang))
            }
            [(sign, at, len @ 1..=2)] if *sign == decimal_sign => {
                let decimals: Vec<String> = chars[*at..*at + *len].iter().map(|d| number(d.to_digit(10).unwrap_or(0) as u64)).collect();
                Some(format!("{} {} {}", number(n), if german { "Komma" } else { "point" }, decimals.join(" ")))
            }
            _ => None,
        };
        let Some(words) = words else {
            out.extend(&chars[i..end]);
            i = end;
            continue;
        };
        out.push_str(&words);
        i = end;
        // what stands right behind the number
        let boundary = |at: usize| chars.get(at).is_none_or(|c| !c.is_alphanumeric());
        let space = usize::from(chars.get(i) == Some(&' '));
        if chars.get(i) == Some(&'%') {
            out.push_str(if german { " Prozent" } else { " percent" });
            i += 1;
        } else if chars.get(i + space) == Some(&'s') && boundary(i + space + 1) {
            out.push_str(if german { " Sekunden" } else { " seconds" });
            i += space + 1;
        } else if chars.get(i).is_some_and(|c| c.is_alphabetic()) {
            out.push(' ');
        }
    }
    out
}

/// The fuel the race still needs, as [`fuel_to_flag`] says it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FuelToFlag {
    Add { laps: u32, litres: u32 },
    Enough { laps: u32 },
    /// More than the tank takes
    FillUp { laps: u32 },
    /// The race's length in laps is not known
    NoDistance,
    /// No fuel per lap is known
    NoFuelUse,
}

/// "Noch neun Runden: zehn Liter nachtanken." / "Nine laps to go: add ten litres.", with "der Sprit reicht" /
/// "fuel is enough", "volltanken" / "fill up", or what is missing.
pub fn fuel_to_flag(advice: FuelToFlag, lang: &str) -> String {
    use FuelToFlag::*;
    if lang == "de" {
        let laps = |n: u32| if n == 1 { "Noch eine Runde".to_string() } else { format!("Noch {} Runden", de_number(n as u64)) };
        match advice {
            Add { laps: n, litres: 1 } => format!("{}: einen Liter nachtanken.", laps(n)),
            Add { laps: n, litres } => format!("{}: {} Liter nachtanken.", laps(n), de_number(litres as u64)),
            Enough { laps: n } => format!("{}, der Sprit reicht.", laps(n)),
            FillUp { laps: n } => format!("{}: volltanken.", laps(n)),
            NoDistance => "Renndistanz unbekannt, keine Spritempfehlung.".to_string(),
            NoFuelUse => "Verbrauch unbekannt, keine Spritempfehlung.".to_string(),
        }
    } else {
        let laps = |n: u32| if n == 1 { "One lap to go".to_string() } else { format!("{} laps to go", capitalised(&en_number(n as u64))) };
        match advice {
            Add { laps: n, litres: 1 } => format!("{}: add one litre.", laps(n)),
            Add { laps: n, litres } => format!("{}: add {} litres.", laps(n), en_number(litres as u64)),
            Enough { laps: n } => format!("{}, fuel is enough.", laps(n)),
            FillUp { laps: n } => format!("{}: fill up.", laps(n)),
            NoDistance => "Race distance unknown, no fuel target.".to_string(),
            NoFuelUse => "Fuel use unknown, no fuel target.".to_string(),
        }
    }
}

/// The time left in the box: "Noch vierunddreißig Sekunden." / "Thirty-four seconds."
pub fn pit_stop_time(seconds: u64, lang: &str) -> String {
    if lang == "de" { format!("Noch {} Sekunden.", de_number(seconds)) } else { format!("{} seconds.", capitalised(&en_number(seconds))) }
}

/// A lap time on its own, as the engineer calls it after a lap: "Acht zwölf vier." / "Eight twelve four."
pub fn lap_time(lap_ms: i64, lang: &str) -> String {
    format!("{}.", capitalised(&lap_time_words(lap_ms, lang)))
}

/// "Neue Bestzeit, acht zwölf vier." / "New personal best, eight twelve four."
pub fn personal_best(lap_ms: i64, lang: &str) -> String {
    let time = lap_time_words(lap_ms, lang);
    if lang == "de" { format!("Neue Bestzeit, {time}.") } else { format!("New personal best, {time}.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coach_s_numbers_are_spelled_out_for_the_voice() {
        assert_eq!(spoken("Deine letzte Runde war 1:12,4, die beste 1:05,37.", "de"), "Deine letzte Runde war eins zwölf vier, die beste eins null fünf drei.");
        assert_eq!(spoken("Your last lap was 8:12.4.", "en"), "Your last lap was eight twelve four.");
        assert_eq!(spoken("Sprit für 4 Runden, 26,4 Liter im Tank.", "de"), "Sprit für vier Runden, sechsundzwanzig Komma vier Liter im Tank.");
        assert_eq!(spoken("Fronts at 27.5 psi, 85 degrees.", "en"), "Fronts at twenty-seven point five psi, eighty-five degrees.");
        assert_eq!(spoken("Noch 12 Minuten, also 1:30 pro Runde.", "de"), "Noch zwölf Minuten, also eins dreißig pro Runde.");
        assert_eq!(spoken("Bremsen bei 640 Grad in Runde 3.", "de"), "Bremsen bei sechshundertvierzig Grad in Runde drei.");
        assert_eq!(spoken("In T13 und nach 1500 Metern.", "de"), "In T13 und nach 1500 Metern.", "part of a word, and too long to say, stay");
        // units stuck to the number, signs, per cent
        assert_eq!(spoken("Du verlierst 0,25s in Kurve 4.", "de"), "Du verlierst null Komma zwei fünf Sekunden in Kurve vier.");
        assert_eq!(spoken("You lose 1.2s there, 0.3 s more than before.", "en"), "You lose one point two seconds there, zero point three seconds more than before.");
        assert_eq!(spoken("Tank bei 85%.", "de"), "Tank bei fünfundachtzig Prozent.");
        assert_eq!(spoken("Fuel at 85%.", "en"), "Fuel at eighty-five percent.");
        assert_eq!(spoken("Das sind -0,3 s und +2 Runden.", "de"), "Das sind minus null Komma drei Sekunden und plus zwei Runden.");
        assert_eq!(spoken("Top-Speed 8km/h höher.", "de"), "Top-Speed acht km/h höher.");
        // what is not a plain number of the language stays as written, whole
        assert_eq!(spoken("Nach 1.000 Metern, am 12.11., Version 0.9.1, 0,125 Sekunden.", "de"), "Nach 1.000 Metern, am 12.11., Version 0.9.1, 0,125 Sekunden.");
        assert_eq!(spoken("After 1,000 metres.", "en"), "After 1,000 metres.");
        assert_eq!(spoken("Reifen 27.5 psi.", "de"), "Reifen 27.5 psi.", "an English decimal in a German sentence is left to the voice");
        assert_eq!(spoken("Alles gut.", "de"), "Alles gut.");
        assert_eq!(spoken("", "en"), "");
    }

    fn all_keys() -> Vec<&'static str> {
        FIXED.iter().chain(&NAMED).chain(&PREVIEW).copied().collect()
    }

    #[test]
    fn the_file_has_exactly_the_keys_of_the_code_and_its_version() {
        let file: serde_json::Value = serde_json::from_str(include_str!("phrases.json")).unwrap();
        assert_eq!(file["version"], VERSION);
        let mut in_file: Vec<&str> = library().phrases.keys().map(String::as_str).collect();
        let mut in_code = all_keys();
        in_file.sort();
        in_code.sort();
        assert_eq!(in_file, in_code);
    }

    #[test]
    fn every_key_has_german_and_english_without_empty_wordings() {
        for key in all_keys() {
            for lang in LANGUAGES {
                let all = wordings(key, lang);
                assert!(!all.is_empty(), "{key} {lang}");
                for w in all {
                    assert_eq!(w.trim(), w, "{key} {lang}: {w:?}");
                    assert!(w.chars().any(char::is_alphabetic), "{key} {lang}: {w:?}");
                    assert!(w.ends_with(['.', '!', '?']), "{key} {lang}: {w:?} ends like a sentence");
                    assert!(!w.chars().any(|c| c.is_ascii_digit()), "{key} {lang}: {w:?} spells its numbers");
                }
            }
        }
    }

    #[test]
    fn the_name_placeholder_is_only_and_always_in_named_and_preview_calls() {
        for key in all_keys() {
            let named = NAMED.contains(&key) || PREVIEW.contains(&key);
            for lang in LANGUAGES {
                for w in wordings(key, lang) {
                    assert_eq!(w.matches(NAME).count(), named as usize, "{key} {lang}: {w:?}");
                    assert!(!w.replace(NAME, "").contains(['{', '}']), "{key} {lang}: {w:?}");
                }
            }
        }
    }

    #[test]
    fn spotter_calls_stay_short() {
        // Beside the driver: at most four words; from behind and ahead (less urgent): at most five.
        for (i, key) in FIXED[..14].iter().enumerate() {
            for lang in LANGUAGES {
                for w in wordings(key, lang) {
                    assert!(w.split_whitespace().count() <= if i < 9 { 4 } else { 5 }, "{key} {lang}: {w:?}");
                }
            }
        }
    }

    #[test]
    fn text_picks_a_wording_and_puts_the_name_in() {
        assert_eq!(text("car_left", "de", None, 0).as_deref(), Some("Auto links."));
        assert_eq!(text("car_left", "en", Some("Max"), 7).as_deref(), Some("Car left."));
        assert_eq!(text("copy_moment", "de", None, 1).as_deref(), Some("Verstanden, ich rechne kurz."));
        assert_eq!(text("copy_moment", "de", None, 5), text("copy_moment", "de", None, 1), "wraps round");
        assert_eq!(text("preview_vocative", "de", Some(" Jörrn "), 0).as_deref(), Some("Jörrn, Box diese Runde."));
        assert_eq!(text("name_left", "en", Some("Max"), 0).as_deref(), Some("Max on your left."));
        assert_eq!(text("name_left", "en", None, 0), None);
        assert_eq!(text("name_left", "en", Some("  "), 0), None);
        assert_eq!(text("car_left", "fr", None, 0), None);
        assert_eq!(text("pit_open", "de", None, 0), None);
    }

    #[test]
    fn cache_name_is_stable_safe_and_differs_per_voice_language_and_text() {
        let a = cache_name("M1", "de", "Auto links.");
        assert_eq!(a, cache_name("M1", "de", "Auto links."));
        assert!(a.starts_with("M1-de-") && a.len() == 6 + 32, "{a}");
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        let others = [cache_name("F1", "de", "Auto links."), cache_name("M1", "en", "Auto links."), cache_name("M1", "de", "Auto rechts.")];
        for o in &others {
            assert_ne!(&a, o);
        }
        assert!(cache_name("../x", "d/e", "t").starts_with("x-de-"));
    }

    #[test]
    fn numbers_are_spelled_out() {
        let de: Vec<String> = [0, 1, 7, 16, 20, 21, 31, 99, 100, 101, 112, 230].map(de_number).to_vec();
        assert_eq!(de, ["null", "eins", "sieben", "sechzehn", "zwanzig", "einundzwanzig", "einunddreißig", "neunundneunzig", "einhundert", "einhunderteins", "einhundertzwölf", "zweihundertdreißig"]);
        let en: Vec<String> = [0, 13, 40, 42, 100, 105, 999].map(en_number).to_vec();
        assert_eq!(en, ["zero", "thirteen", "forty", "forty-two", "one hundred", "one hundred and five", "nine hundred and ninety-nine"]);
        assert_eq!(de_number(1000), "1000");
    }

    #[test]
    fn fuel_laps_rounds_down_to_half_a_lap() {
        let de = |l| fuel_laps(l, "de");
        let en = |l| fuel_laps(l, "en");
        assert_eq!(de(4.0), "Sprit für noch vier Runden.");
        assert_eq!(de(4.99), "Sprit für noch viereinhalb Runden.");
        assert_eq!(de(1.2), "Sprit für noch eine Runde.");
        assert_eq!(de(1.5), "Sprit für noch anderthalb Runden.");
        assert_eq!(de(0.7), "Sprit für noch eine halbe Runde.");
        assert_eq!(de(0.3), "Sprit für weniger als eine halbe Runde.");
        assert_eq!(de(-1.0), "Sprit für weniger als eine halbe Runde.");
        assert_eq!(de(f64::NAN), "Sprit für weniger als eine halbe Runde.");
        assert_eq!(de(2.5), "Sprit für noch zweieinhalb Runden.");
        assert_eq!(de(21.5), "Sprit für noch einundzwanzigeinhalb Runden.");
        assert_eq!(de(101.5), "Sprit für noch einhunderteineinhalb Runden.");
        assert_eq!(de(21.0), "Sprit für noch einundzwanzig Runden.");
        assert_eq!(en(4.0), "Fuel for four more laps.");
        assert_eq!(en(1.0), "Fuel for one more lap.");
        assert_eq!(en(1.5), "Fuel for one and a half more laps.");
        assert_eq!(en(0.5), "Fuel for half a lap.");
        assert_eq!(en(0.1), "Fuel for less than half a lap.");
        assert_eq!(en(21.9), "Fuel for twenty-one and a half more laps.");
    }

    #[test]
    fn the_fuel_to_the_flag() {
        use FuelToFlag::*;
        let de = |advice| fuel_to_flag(advice, "de");
        let en = |advice| fuel_to_flag(advice, "en");
        assert_eq!(de(Add { laps: 9, litres: 10 }), "Noch neun Runden: zehn Liter nachtanken.");
        assert_eq!(de(Add { laps: 1, litres: 1 }), "Noch eine Runde: einen Liter nachtanken.");
        assert_eq!(de(Enough { laps: 21 }), "Noch einundzwanzig Runden, der Sprit reicht.");
        assert_eq!(de(FillUp { laps: 30 }), "Noch dreißig Runden: volltanken.");
        assert_eq!(de(NoDistance), "Renndistanz unbekannt, keine Spritempfehlung.");
        assert_eq!(de(NoFuelUse), "Verbrauch unbekannt, keine Spritempfehlung.");
        assert_eq!(en(Add { laps: 9, litres: 10 }), "Nine laps to go: add ten litres.");
        assert_eq!(en(Add { laps: 1, litres: 1 }), "One lap to go: add one litre.");
        assert_eq!(en(Enough { laps: 21 }), "Twenty-one laps to go, fuel is enough.");
        assert_eq!(en(FillUp { laps: 30 }), "Thirty laps to go: fill up.");
        assert_eq!(en(NoDistance), "Race distance unknown, no fuel target.");
        assert_eq!(en(NoFuelUse), "Fuel use unknown, no fuel target.");
    }

    #[test]
    fn the_time_left_in_the_box() {
        assert_eq!(pit_stop_time(34, "de"), "Noch vierunddreißig Sekunden.");
        assert_eq!(pit_stop_time(85, "de"), "Noch fünfundachtzig Sekunden.");
        assert_eq!(pit_stop_time(120, "de"), "Noch einhundertzwanzig Sekunden.");
        assert_eq!(pit_stop_time(30, "en"), "Thirty seconds.");
        assert_eq!(pit_stop_time(85, "en"), "Eighty-five seconds.");
    }

    #[test]
    fn lap_times_are_said_like_a_race_engineer() {
        let de = |ms| lap_time(ms, "de");
        let en = |ms| lap_time(ms, "en");
        assert_eq!(de(492_400), "Acht zwölf vier.");
        assert_eq!(en(492_400), "Eight twelve four.");
        assert_eq!(de(65_300), "Eins null fünf drei.");
        assert_eq!(en(65_300), "One oh five three.");
        assert_eq!(de(60_400), "Eins null null vier.");
        assert_eq!(en(60_400), "One oh oh four.");
        assert_eq!(de(58_300), "Achtundfünfzig drei.");
        assert_eq!(en(58_300), "Fifty-eight three.");
        assert_eq!(de(9_500), "Neun fünf.");
        assert_eq!(de(132_000), "Zwei zwölf null.");
        assert_eq!(en(132_000), "Two twelve zero.");
        assert_eq!(en(83_250), "One twenty-three two.");
        // Cut, not rounded: hundredths never lift the tenth or the minute.
        assert_eq!(de(72_399), "Eins zwölf drei.");
        assert_eq!(de(59_999), "Neunundfünfzig neun.");
        assert_eq!(de(600_100), "Zehn null null eins.");
        assert_eq!(de(-5), "Null null.");
        assert_eq!(personal_best(492_400, "de"), "Neue Bestzeit, acht zwölf vier.");
        assert_eq!(personal_best(65_300, "en"), "New personal best, one oh five three.");
    }
}
