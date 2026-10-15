//! The game's MoTeC sessions, which the app uploads to the platform when the driver switched on the MoTeC upload
//! (Telemetry). The game writes them to `Saved Games\ACE\MoTec\<car>\<track>\<name>.ld` (with an `.ldx` next to
//! each) and a session is cumulative: every lap writes a new, larger file, and the newest holds all laps. So per
//! session only the newest `.ld` goes up, gzipped, once the session has ended, and never one older than 30 days.
//! By hand (the page's "Upload now") a running session goes up too, as long as the car is not driven (it may stand
//! in its pit box, where the game writes the files).
//! This holds the rules; the shell lists the folder, gzips and talks to the platform
//! (`/api/telemetry/motec/upload` and `/complete`, plan 2026-10-13-runden-aller-autos-handoff.md section 5).

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The game's MoTeC folder in its `Saved Games\ACE`.
pub const MOTEC_DIR: &str = "MoTec";
/// While the game runs, a session counts as running until no file was written for this long.
pub const QUIET_SECS: u64 = 10 * 60;
/// Two files of a session are written at most this far apart (tlm_sessions.py `CHAIN_GAP`).
pub const CHAIN_GAP_SECS: u64 = 30 * 60;
/// Older sessions are never uploaded: the first start must not push a whole archive.
pub const MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
/// A smaller `.ld` is only the header (0 laps of telemetry, about 19 KB).
pub const HEADER_ONLY: u64 = 64 * 1024;
/// The platform takes at most this much per request.
pub const CHUNK: u64 = 8 * 1024 * 1024;
/// The platform takes an `.ldx` of at most this size with the completion.
pub const LDX_MAX: u64 = 64 * 1024;
const NAME_MAX: usize = 255;
/// The app's record of what it uploaded, in its config folder.
const LEDGER: &str = "motec-uploaded.json";

/// An `.ld` file in the MoTeC folder: its path below the folder with forward slashes (`<car>/<track>/<file>.ld`),
/// its size and when it was written (seconds since 1970).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotecFile {
    pub name: String,
    pub size: u64,
    pub modified: u64,
}

/// Whether the platform takes `name` as the name of an upload: `<car>/<track>/<file>.ld`, three plain segments,
/// at most 255 characters.
pub fn is_upload_name(name: &str) -> bool {
    let segments: Vec<&str> = name.split('/').collect();
    name.chars().count() <= NAME_MAX
        && name.ends_with(".ld")
        && !name.contains("..")
        && !name.contains('\\')
        && segments.len() == 3
        && segments.iter().all(|s| !s.is_empty())
}

/// What makes files one session: the folder (car and track) and the name without the time the game appends
/// (`<car>_preset_<preset>_<track>`), which in one folder is the preset, as tlm_sessions.py keys it.
fn session_key(name: &str) -> (&str, &str) {
    let (folder, file) = name.rsplit_once('/').unwrap_or(("", name));
    let stem = file.strip_suffix(".ld").unwrap_or(file);
    (folder, stem.rsplit_once('_').map_or(stem, |(key, _time)| key))
}

/// The sessions, as tlm_sessions.py `chains` finds them, in the order they started: the files by time, and a file
/// belongs to the last session of its key when it is at least as large as that session's newest file and written
/// at most `CHAIN_GAP_SECS` after it; otherwise it starts a new one.
pub fn sessions(files: &[MotecFile]) -> Vec<Vec<&MotecFile>> {
    let mut sorted: Vec<&MotecFile> = files.iter().collect();
    sorted.sort_by(|a, b| (a.modified, &a.name).cmp(&(b.modified, &b.name)));
    let mut chains: Vec<Vec<&MotecFile>> = Vec::new();
    for file in sorted {
        let key = session_key(&file.name);
        let last = chains.iter_mut().rev().find(|chain| session_key(&chain[0].name) == key);
        match last {
            Some(chain) if file.size >= chain[chain.len() - 1].size && file.modified - chain[chain.len() - 1].modified <= CHAIN_GAP_SECS => chain.push(file),
            _ => chains.push(vec![file]),
        }
    }
    chains
}

/// Whether a session is running: the game runs and wrote a MoTeC file in the last `QUIET_SECS`. Then nothing is
/// uploaded, so the upload never competes with the game.
pub fn busy(files: &[MotecFile], now: u64, game_running: bool) -> bool {
    game_running && files.iter().any(|f| now.saturating_sub(f.modified) < QUIET_SECS)
}

/// Why a call of the upload sends nothing, for the page and the app log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The call before still uploads.
    Running,
    /// Every session is uploaded.
    NothingWaiting,
    /// Uploads pause for a while after a full quota or a file the platform found broken.
    Paused,
    /// The game drives live; by hand only with the car out of its pit box.
    Driving,
    /// The game runs and wrote a MoTeC file in the last `QUIET_SECS`; only the automatic upload waits for that.
    Quiet,
    /// The file changed while the app looked at it: the game is writing it.
    Writing,
}

impl Hold {
    /// Its name in the command's answer.
    pub fn code(self) -> &'static str {
        match self {
            Hold::Running => "running",
            Hold::NothingWaiting => "nothing-waiting",
            Hold::Paused => "paused",
            Hold::Driving => "driving",
            Hold::Quiet => "quiet",
            Hold::Writing => "writing",
        }
    }

    /// What the app log says.
    pub fn reason(self) -> &'static str {
        match self {
            Hold::Running => "the call before still uploads",
            Hold::NothingWaiting => "every session is uploaded",
            Hold::Paused => "paused after a full quota or a broken file",
            Hold::Driving => "the game drives live",
            Hold::Quiet => "the game wrote a MoTeC file in the last 10 minutes",
            Hold::Writing => "the game is still writing the file",
        }
    }

    /// Whether the page should try again soon: not when nothing is to do, or not for hours.
    pub fn busy(self) -> bool {
        !matches!(self, Hold::NothingWaiting | Hold::Paused)
    }
}

/// Why the oldest of `waiting` sessions may not go up now, None when it may. By hand (`manual`) a running session
/// goes up too, but nothing goes up while the car is driven (`driving`: the automatic upload counts live driving,
/// by hand only the car out of its pit box).
pub fn hold(files: &[MotecFile], waiting: usize, now: u64, game_running: bool, driving: bool, paused: bool, manual: bool) -> Option<Hold> {
    if waiting == 0 {
        Some(Hold::NothingWaiting)
    } else if paused {
        Some(Hold::Paused)
    } else if driving {
        Some(Hold::Driving)
    } else if !manual && busy(files, now, game_running) {
        Some(Hold::Quiet)
    } else {
        None
    }
}

/// The newest file of every session still to upload, the oldest first: not uploaded yet (`uploaded`), more than a
/// header, a name the platform takes, and written in the last `MAX_AGE_SECS`.
pub fn pending<'a>(files: &'a [MotecFile], uploaded: &[Uploaded], now: u64) -> Vec<&'a MotecFile> {
    let mut out: Vec<&MotecFile> = sessions(files)
        .into_iter()
        .filter_map(|chain| chain.last().copied())
        .filter(|f| f.size >= HEADER_ONLY && now.saturating_sub(f.modified) <= MAX_AGE_SECS && is_upload_name(&f.name))
        .filter(|f| !uploaded.iter().any(|u| u.name == f.name && u.size == f.size))
        .collect();
    out.sort_by(|a, b| (a.modified, &a.name).cmp(&(b.modified, &b.name)));
    out
}

/// A file the app uploaded (or the platform had already), by name and size, and when it went up (seconds since
/// 1970); older records, and files the app did not send itself, have no time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uploaded {
    pub name: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// What the app uploaded from this PC; nothing when the record is missing or unreadable (the platform then answers
/// `done` for what it has).
pub fn uploaded(dir: &Path) -> Vec<Uploaded> {
    fs::read(dir.join(LEDGER)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn record_uploaded(dir: &Path, name: &str, size: u64, at: Option<u64>) -> io::Result<()> {
    let mut all = uploaded(dir);
    if !all.iter().any(|u| u.name == name && u.size == size) {
        all.push(Uploaded { name: name.to_string(), size, at });
    }
    fs::create_dir_all(dir)?;
    fs::write(dir.join(LEDGER), serde_json::to_vec(&all).map_err(io::Error::other)?)
}

/// The address of a step of the upload (`upload` or `complete`) on the platform at `base`, with the ticket, the
/// name and, for a chunk, its offset.
pub fn endpoint(base: &str, step: &str, ticket: &str, name: &str, offset: Option<u64>) -> Option<String> {
    let mut url = url::Url::parse(base).ok().filter(|url| matches!(url.scheme(), "http" | "https"))?.join(&format!("/api/telemetry/motec/{step}")).ok()?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("ticket", ticket).append_pair("name", name);
        if let Some(offset) = offset {
            query.append_pair("offset", &offset.to_string());
        }
    }
    Some(url.into())
}

/// The next chunk to send of a file of `len` bytes when the platform has `offset` of them: where it starts and how
/// long it is. None when the platform has them all.
pub fn chunk(offset: u64, len: u64) -> Option<(u64, u64)> {
    (offset < len).then(|| (offset, CHUNK.min(len - offset)))
}

/// How much of an upload the platform has (`GET` and `PUT` of `/upload`), and whether it was completed already.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct Received {
    pub size: u64,
    #[serde(default)]
    pub done: bool,
}

/// The platform's reason of a refused request (`data.problem`, empty without one) and, for `motec-offset`, the
/// size it has (`data.size`).
pub fn problem(body: &[u8]) -> (String, Option<u64>) {
    let json: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let data = &json["data"];
    (data["problem"].as_str().unwrap_or_default().to_string(), data["size"].as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An excerpt of Joe's MoTeC folder as the shell lists it, not in time order (2026-10-13; the time written in
    /// seconds since 1970, the size, the name): two GT2 RS MR sessions, a Rennsport stage 2 evening with a restart
    /// in the middle and two equal files, stage 3, and two Kunos cars, one with header-only files and a day of
    /// the month with two spaces in the name.
    const LISTING: &str = "\
1791837405 34850204 Porsche 911 GT2 RS MR/NurburgringTouristenfahrten/Porsche 911 GT2 RS MR_preset_dd_porsche_gt2rs_mr_stage2_NurburgringTouristenfahrten_Mon Oct 12 213645 2026.ld
1791837649 40356336 Porsche 911 GT2 RS MR/NurburgringTouristenfahrten/Porsche 911 GT2 RS MR_preset_dd_porsche_gt2rs_mr_stage2_NurburgringTouristenfahrten_Mon Oct 12 214049 2026.ld
1791837748 44501456 Porsche 911 GT2 RS MR/NurburgringTouristenfahrten/Porsche 911 GT2 RS MR_preset_dd_porsche_gt2rs_mr_stage2_NurburgringTouristenfahrten_Mon Oct 12 214228 2026.ld
1791838156 1640720 Porsche 911 GT2 RS MR/NurburgringTouristenfahrten/Porsche 911 GT2 RS MR_preset_dd_porsche_gt2rs_mr_stage2_NurburgringTouristenfahrten_Mon Oct 12 214916 2026.ld
1791838872 6361444 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 220112 2026.ld
1791839344 9296792 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 220904 2026.ld
1791840132 26309924 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 222212 2026.ld
1791840584 29281132 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 222944 2026.ld
1791840688 29281132 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 223128 2026.ld
1791841938 6304420 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 225218 2026.ld
1791842103 6304420 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 225503 2026.ld
1791842572 16483220 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage2_NurburgringTouristenfahrten_Mon Oct 12 230252 2026.ld
1791843882 15625928 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage3_NurburgringTouristenfahrten_Mon Oct 12 232442 2026.ld
1791844330 17346680 BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_stage3_NurburgringTouristenfahrten_Mon Oct 12 233210 2026.ld
1791683476 19520 Porsche 911 GT2 RS Clubsport EVO/NurburgringTouristenfahrten/Porsche 911 GT2 RS Clubsport EVO_preset_gt2rscs_mech_1_NurburgringTouristenfahrten_Sun Oct 11 025116 2026.ld
1791683966 19520 Porsche 911 GT2 RS Clubsport EVO/NurburgringTouristenfahrten/Porsche 911 GT2 RS Clubsport EVO_preset_gt2rscs_mech_1_NurburgringTouristenfahrten_Sun Oct 11 025926 2026.ld
1791684383 19520 Porsche 911 GT2 RS Clubsport EVO/NurburgringTouristenfahrten/Porsche 911 GT2 RS Clubsport EVO_preset_gt2rscs_mech_1_NurburgringTouristenfahrten_Sun Oct 11 030623 2026.ld
1791834950 19520 Porsche 911 GT2 RS Clubsport EVO/NurburgringTouristenfahrten/Porsche 911 GT2 RS Clubsport EVO_preset_gt2rscs_mech_1_NurburgringTouristenfahrten_Mon Oct 12 205550 2026.ld
1791845982 2166212 Porsche 911 GT2 RS Clubsport EVO/NurburgringTouristenfahrten/Porsche 911 GT2 RS Clubsport EVO_preset_gt2rscs_mech_1_NurburgringTouristenfahrten_Mon Oct 12 235942 2026.ld
1791053487 43250960 BMW M4 GT3 Evo/NurburgringTouristenfahrten/BMW M4 GT3 Evo_preset_m4gt3_mech_1_NurburgringTouristenfahrten_Sat Oct  3 195127 2026.ld
1791053683 43250960 BMW M4 GT3 Evo/NurburgringTouristenfahrten/BMW M4 GT3 Evo_preset_m4gt3_mech_1_NurburgringTouristenfahrten_Sat Oct  3 195443 2026.ld
";

    /// The listing above.
    fn listing() -> Vec<MotecFile> {
        LISTING
            .lines()
            .map(|line| {
                let mut parts = line.splitn(3, ' ');
                let modified = parts.next().unwrap().parse().unwrap();
                let size = parts.next().unwrap().parse().unwrap();
                MotecFile { name: parts.next().unwrap().to_string(), size, modified }
            })
            .collect()
    }

    /// When the listing's last file was written.
    const NEWEST: u64 = 1791845982;

    /// The time in the names of a session's files, first to last.
    fn times(chain: &[&MotecFile]) -> Vec<String> {
        chain.iter().map(|f| f.name.rsplit_once('_').unwrap().1.trim_end_matches(".ld").to_string()).collect()
    }

    #[test]
    fn finds_the_sessions_as_tlm_sessions_does() {
        let files = listing();
        let found: Vec<Vec<String>> = sessions(&files).iter().map(|chain| times(chain)).collect();
        assert_eq!(found, [
            // the M4 GT3: equal sizes stay one session
            vec!["Sat Oct  3 195127 2026", "Sat Oct  3 195443 2026"],
            // the GT2 RS Clubsport: three headers in 20 minutes, then one three hours later
            vec!["Sun Oct 11 025116 2026", "Sun Oct 11 025926 2026", "Sun Oct 11 030623 2026"],
            vec!["Mon Oct 12 205550 2026"],
            // the GT2 RS MR: three growing files, then a smaller one starts a new session
            vec!["Mon Oct 12 213645 2026", "Mon Oct 12 214049 2026", "Mon Oct 12 214228 2026"],
            vec!["Mon Oct 12 214916 2026"],
            // stage 2: growing, then the restart at 22:52 is smaller
            vec!["Mon Oct 12 220112 2026", "Mon Oct 12 220904 2026", "Mon Oct 12 222212 2026", "Mon Oct 12 222944 2026", "Mon Oct 12 223128 2026"],
            vec!["Mon Oct 12 225218 2026", "Mon Oct 12 225503 2026", "Mon Oct 12 230252 2026"],
            // stage 3 is another preset, so another session although it is larger and written soon after
            vec!["Mon Oct 12 232442 2026", "Mon Oct 12 233210 2026"],
            // the Clubsport again, more than half an hour after its last header
            vec!["Mon Oct 12 235942 2026"],
        ]);
    }

    #[test]
    fn a_file_written_more_than_half_an_hour_after_the_last_starts_a_new_session() {
        let file = |size, modified| MotecFile { name: format!("car/track/car_preset_x_track_{modified}.ld"), size, modified };
        let files = [file(100_000, 0), file(200_000, CHAIN_GAP_SECS), file(300_000, 2 * CHAIN_GAP_SECS + 1)];
        assert_eq!(sessions(&files).iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
        // another folder with the same name is another session
        let other = MotecFile { name: "car/other/car_preset_x_track_1.ld".to_string(), size: 300_000, modified: 1 };
        assert_eq!(sessions(&[file(100_000, 0), other]).len(), 2);
    }

    #[test]
    fn uploads_the_newest_file_of_each_session_oldest_first_without_headers() {
        let files = listing();
        let next: Vec<String> = pending(&files, &[], NEWEST).iter().map(|f| times(&[f])[0].clone()).collect();
        // the Clubsport's header-only sessions (19 KB) are not uploaded; 21:49 (1.6 MB) is a session of its own
        assert_eq!(next, [
            "Sat Oct  3 195443 2026", "Mon Oct 12 214228 2026", "Mon Oct 12 214916 2026", "Mon Oct 12 223128 2026", "Mon Oct 12 230252 2026", "Mon Oct 12 233210 2026",
            "Mon Oct 12 235942 2026",
        ]);
        let second = pending(&files, &[], NEWEST)[1].clone();
        assert_eq!(second.name, "Porsche 911 GT2 RS MR/NurburgringTouristenfahrten/Porsche 911 GT2 RS MR_preset_dd_porsche_gt2rs_mr_stage2_NurburgringTouristenfahrten_Mon Oct 12 214228 2026.ld");
        // uploaded ones are left out, by name and size
        let done = [Uploaded { name: second.name.clone(), size: second.size, at: None }, Uploaded { name: files[3].name.clone(), size: 1, at: None }];
        assert_eq!(pending(&files, &done, NEWEST).len(), 6);
        // a session whose newest file is older than 30 days is never uploaded: the M4 GT3's goes first
        assert_eq!(pending(&files, &[], 1791053683 + MAX_AGE_SECS).len(), 7);
        assert_eq!(pending(&files, &[], 1791053683 + MAX_AGE_SECS + 1).len(), 6);
        assert!(pending(&files, &[], NEWEST + MAX_AGE_SECS + 1).is_empty());
    }

    #[test]
    fn a_header_is_less_than_64_kib() {
        let file = |size| MotecFile { name: "c/t/c_preset_p_t_x.ld".to_string(), size, modified: 0 };
        assert!(pending(&[file(HEADER_ONLY - 1)], &[], 0).is_empty());
        assert_eq!(pending(&[file(HEADER_ONLY)], &[], 0).len(), 1);
    }

    #[test]
    fn busy_while_the_game_runs_and_wrote_a_file_in_the_last_ten_minutes() {
        let files = listing();
        assert!(busy(&files, NEWEST + QUIET_SECS - 1, true));
        assert!(!busy(&files, NEWEST + QUIET_SECS, true));
        assert!(!busy(&files, NEWEST, false), "the game is closed: the session ended");
        assert!(!busy(&[], NEWEST, true));
    }

    /// The listing as it was when the file written at `until` was the newest.
    fn listing_until(until: u64) -> Vec<MotecFile> {
        listing().into_iter().filter(|f| f.modified <= until).collect()
    }

    /// When stage 3's two files were written (23:24:42 and 23:32:10).
    const STAGE3: [u64; 2] = [1791843882, 1791844330];

    #[test]
    fn by_hand_the_newest_file_of_a_running_session_goes_up_but_not_while_driving() {
        // stage 3 runs: the game wrote its second file a minute ago
        let files = listing_until(STAGE3[1]);
        let now = STAGE3[1] + 60;
        assert!(busy(&files, now, true));
        let waiting = pending(&files, &[], now);
        assert_eq!(times(&[waiting[waiting.len() - 1]]), ["Mon Oct 12 233210 2026"], "the newest file of the running session, not 23:24:42");
        assert_eq!(hold(&files, waiting.len(), now, true, false, false, false), Some(Hold::Quiet), "the automatic upload waits");
        assert_eq!(hold(&files, waiting.len(), now, true, false, false, true), None, "by hand it goes up");
        assert_eq!(hold(&files, waiting.len(), now, true, true, false, true), Some(Hold::Driving));
        // ten quiet minutes later the automatic upload takes it too
        assert_eq!(hold(&files, waiting.len(), STAGE3[1] + QUIET_SECS, true, false, false, false), None);
    }

    #[test]
    fn says_why_nothing_goes_up_in_this_order() {
        let files = listing();
        let now = NEWEST;
        assert_eq!(hold(&files, 0, now, true, true, true, false), Some(Hold::NothingWaiting));
        assert_eq!(hold(&files, 1, now, true, true, true, false), Some(Hold::Paused));
        assert_eq!(hold(&files, 1, now, true, true, false, false), Some(Hold::Driving));
        assert_eq!(hold(&files, 1, now, true, false, false, false), Some(Hold::Quiet));
        assert_eq!(hold(&files, 1, now, false, false, false, false), None, "the game is closed");
        assert_eq!(hold(&files, 1, now, true, false, true, true), Some(Hold::Paused), "by hand too");
        let all = [Hold::Running, Hold::NothingWaiting, Hold::Paused, Hold::Driving, Hold::Quiet, Hold::Writing];
        assert_eq!(all.map(Hold::code), ["running", "nothing-waiting", "paused", "driving", "quiet", "writing"]);
        assert_eq!(all.map(Hold::busy), [true, false, false, true, true, true]);
    }

    #[test]
    fn takes_only_names_the_platform_takes() {
        assert!(is_upload_name("Porsche 911 GT2 RS MR/NurburgringTouristenfahrten/Porsche 911 GT2 RS MR_preset_dd_porsche_gt2rs_mr_base_NurburgringTouristenfahrten_Mon Oct 12 204344 2026.ld"));
        for name in ["a/b.ld", "a/b/c/d.ld", "/a/b.ld", "a//b.ld", "a/b/c.ldx", "a/../c.ld", "a/b\\x/c.ld", "a/b/c..ld"] {
            assert!(!is_upload_name(name), "{name}");
        }
        assert!(is_upload_name(&format!("a/b/{}.ld", "ä".repeat(248))));
        assert!(!is_upload_name(&format!("a/b/{}.ld", "ä".repeat(249))));
        let long = MotecFile { name: format!("a/b/{}_x.ld", "c".repeat(260)), size: HEADER_ONLY, modified: 0 };
        assert!(pending(&[long], &[], 0).is_empty());
    }

    #[test]
    fn keeps_what_it_uploaded() {
        let dir = std::env::temp_dir().join(format!("dd-motec-{}", std::process::id())).join("config");
        assert!(uploaded(&dir).is_empty());
        record_uploaded(&dir, "a/b/c.ld", 70_000, None).unwrap();
        record_uploaded(&dir, "a/b/c.ld", 70_000, Some(1)).unwrap();
        record_uploaded(&dir, "a/b/d.ld", 80_000, Some(1791902548)).unwrap();
        assert_eq!(uploaded(&dir), [Uploaded { name: "a/b/c.ld".into(), size: 70_000, at: None }, Uploaded { name: "a/b/d.ld".into(), size: 80_000, at: Some(1791902548) }]);
        assert_eq!(fs::read_to_string(dir.join(LEDGER)).unwrap(), r#"[{"name":"a/b/c.ld","size":70000},{"name":"a/b/d.ld","size":80000,"at":1791902548}]"#);
        // a record of 0.23.0, without times, still reads
        fs::write(dir.join(LEDGER), br#"[{"name":"a/b/c.ld","size":70000}]"#).unwrap();
        assert_eq!(uploaded(&dir), [Uploaded { name: "a/b/c.ld".into(), size: 70_000, at: None }]);
        // an unreadable record counts as empty, and is replaced by the next one
        fs::write(dir.join(LEDGER), b"{").unwrap();
        assert!(uploaded(&dir).is_empty());
        record_uploaded(&dir, "a/b/c.ld", 70_000, None).unwrap();
        assert_eq!(uploaded(&dir).len(), 1);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn builds_the_addresses_of_the_upload() {
        let name = "Porsche 911 GT2 RS MR/Nurburgring/a+b_Mon Oct 12 204344 2026.ld";
        assert_eq!(
            endpoint("https://digitaldrivers.club", "upload", "123.abc", name, None).unwrap(),
            "https://digitaldrivers.club/api/telemetry/motec/upload?ticket=123.abc&name=Porsche+911+GT2+RS+MR%2FNurburgring%2Fa%2Bb_Mon+Oct+12+204344+2026.ld"
        );
        assert_eq!(endpoint("http://localhost:3000/", "upload", "t", "a/b/c.ld", Some(8_388_608)).unwrap(), "http://localhost:3000/api/telemetry/motec/upload?ticket=t&name=a%2Fb%2Fc.ld&offset=8388608");
        assert_eq!(endpoint("https://digitaldrivers.club/garage", "complete", "t", "a/b/c.ld", None).unwrap(), "https://digitaldrivers.club/api/telemetry/motec/complete?ticket=t&name=a%2Fb%2Fc.ld");
        assert_eq!(endpoint("file:///c:/x", "upload", "t", "a/b/c.ld", None), None);
        assert_eq!(endpoint("not a url", "upload", "t", "a/b/c.ld", None), None);
    }

    #[test]
    fn sends_8_mib_chunks_from_where_the_platform_is() {
        assert_eq!(chunk(0, 20 * 1024 * 1024), Some((0, CHUNK)));
        assert_eq!(chunk(2 * CHUNK, 20 * 1024 * 1024), Some((2 * CHUNK, 4 * 1024 * 1024)));
        assert_eq!(chunk(20 * 1024 * 1024, 20 * 1024 * 1024), None);
        assert_eq!(chunk(21 * 1024 * 1024, 20 * 1024 * 1024), None, "the platform has more than the file: completing fails and starts it over");
        assert_eq!(chunk(0, 0), None);
    }

    #[test]
    fn reads_the_platform_s_answers() {
        assert_eq!(serde_json::from_slice::<Received>(br#"{"size":8388608,"done":false}"#).unwrap(), Received { size: 8_388_608, done: false });
        assert_eq!(serde_json::from_slice::<Received>(br#"{"size":0,"done":true}"#).unwrap(), Received { size: 0, done: true });
        assert_eq!(serde_json::from_slice::<Received>(br#"{"size":16777216}"#).unwrap(), Received { size: 16_777_216, done: false });
        // h3's error answer as the platform sends it (createError with data)
        let conflict = br#"{"url":"/api/telemetry/motec/upload","statusCode":409,"statusMessage":"motec-offset","message":"motec-offset","data":{"problem":"motec-offset","size":8388608}}"#;
        assert_eq!(problem(conflict), ("motec-offset".to_string(), Some(8_388_608)));
        assert_eq!(problem(br#"{"statusCode":413,"data":{"problem":"motec-quota"}}"#), ("motec-quota".to_string(), None));
        assert_eq!(problem(b"<html>Bad gateway</html>"), (String::new(), None));
    }
}
