# dd-desktop

Digital Drivers desktop app for Windows. Joins races with one click, runs technical scrutineering on cars
and tracks, installs content and car setups. Open-source shell built with Tauri.

The app is a thin shell: it loads the hosted Digital Drivers interface and adds a small set of native
commands. It contains no business logic; decisions are made on the server. The source is public so every
driver can check what the app does on their PC.

## What the app touches on your PC

- It reads Steam's install path from the registry and Steam's `libraryfolders.vdf` to find Assetto Corsa.
- The system check writes a tiny probe file (`dd-desktop-write-probe.tmp`) into the Assetto Corsa folder and
  removes it right away, to learn whether content can be installed there.
- It reads which account is signed in to Steam (`ActiveProcess\ActiveUser` in Steam's registry key): a slot on a
  race server is reserved for one SteamID, and the app says so when Steam runs with another account.
- When you join a race, it asks that race server for your slot (the livery, and the Custom Shaders Patch features
  the server announces), writes the game's `Documents\Assetto Corsa\cfg\race.ini` for that session and starts
  `acs.exe`. Every launcher rewrites `race.ini` for every session; your unit of speed and nationality are kept.
  If the game folder has no `steam_appid.txt` yet, the app creates it (content `244210`, the game's Steam id):
  without it Steam starts the game's own launcher instead of the session. Content Manager creates the same file.
- For technical scrutineering it reads the game files the platform asks about (the data of your car, the
  surfaces and models of the track: only files below the game's `content` and `system` folders) and reports
  their SHA-256, and it reads the build number of your Custom Shaders Patch
  (`extension\config\data_manifest.ini`). The files themselves never leave your PC.
- For a race against bots it writes the game's `race.ini` for a single-player race, reads the names of your
  car's liveries so the bots get different ones, and reads the game's own result file
  (`Documents\Assetto Corsa\out\race_out.json`) once the game has closed.
- The system check reads which program is registered for Content Manager's `acmanager://` links
  (`Software\Classes\acmanager\shell\open\command`) and whether that file exists, and the build number of your
  Custom Shaders Patch, so the platform can say what is missing before you try to join.
- The system check looks for Assetto Corsa EVO in your Steam libraries and for the folder the game keeps your
  files in (`Saved Games\ACE`; where that is when you moved `Saved Games`, Windows notes in the registry under
  `Explorer\User Shell Folders`).
- When you put one of the club's cars into the game (the garage), the app downloads that car's package from
  the platform, checks its SHA-256 and writes it as `Saved Games\ACE\mods\<id>.kspkg`, never while the game
  runs; while the app is open it updates the cars you have the same way, once an hour. To report which cars you
  have, it hashes the packages of the names the platform asks about. After an update it reads the saved cars of
  that car in `ProfileData\<profile>\OpenData\SavedCars`: one that points at a part the new version no longer
  has moves to `SavedCars\stale` (the game would crash on start), and if your garage had selected it,
  `garage.drivergarage` is saved as `.bak` and then selects the Kunos Porsche 911 GT3 Cup.
- When you install a car setup of the platform for Assetto Corsa EVO, the app writes that one file into the
  game's setup folder (`Saved Games\ACE\Car Setups\<car>\<track>\<name>.carsetup`), where the game's setup
  screen lists it. To show which setups you have already, it reads the files of the names the platform asks
  about in that folder and reports their SHA-256. A setup you changed and saved under the same name is not
  overwritten, unless you ask for the original. Nothing else in the folder is read, changed or removed.
- A link the platform opens in a new window (a stream, a download) goes to your default browser; the app has no
  tabs. Only web addresses (`http`, `https`) are passed on, anything else is refused.
- Installed with the `setup.exe`, the app asks GitHub at every start for the newest release (one request to
  `github.com`, the update manifest of this repository's releases). A newer installer is downloaded, checked
  against the signing key built into the app and run; it replaces the app and starts it again. The Store build
  does not do this: the Store keeps it up to date.
- It writes a log to `%TEMP%\dd-desktop.log`.

Nothing else is read, and nothing is uploaded by the shell itself.

## How it works

| Part | What |
| --- | --- |
| `src/` | Bundled start page: checks that the platform is reachable, then opens it. Offline it shows a system check. |
| `src-tauri/` | The Tauri shell with the native commands `platform_url`, `system_check`, `show_toast`, `join_race`, `scrutineer`, `start_bot_race`, `bot_race_result`, `setup_status`, `install_setup`, `update_check`, `update_install`, `launch_ac_evo`, `car_status` and `install_car`. |
| `src-tauri/capabilities/` | Which page may call which command. The hosted interface only gets the commands listed in `platform.json`; `dev-localhost.json` is enabled for development builds only. |
| `crates/dd-core/` | Platform-independent logic (locating Assetto Corsa and Assetto Corsa EVO, the join ticket and the `race.ini` of an online session, hashing game files for scrutineering, the race against bots, car setups), unit-tested on any machine. |

Commands must be declared in `src-tauri/build.rs` and allowed per origin in a capability file. A hosted page
cannot call anything that is not listed for its origin.

`show_toast` shows a Windows notification (plain title and text, cut to a sane length). The hosted interface
calls it for new notifications of the signed-in driver, because a WebView has no push service. Windows drops
toasts of an app id it does not know, so the shell picks the id by how it runs: the package's own id when
installed from the Store or as MSIX, the Tauri identifier for an installed build, and PowerShell's id for a
build started from the `target` folder.

`join_race` takes a ticket from the hosted interface (race server address and ports, track, car, the driver's
name and SteamID) and starts the game on that server. Every value is checked before it is used: content names
may only be folder names, free text becomes one line, so a ticket can neither leave the content folder nor add
keys to `race.ini`. The command refuses when Steam is not running, when Steam runs with another account than
the ticket names, when the car or the track is not installed, or when the race server has no free slot for the
driver with that car, and answers with a short code the interface has the words for.

`scrutineer` takes a list of paths and answers with the SHA-256 of each file (or that it is not there), the
build of the Custom Shaders Patch and the app's version. It judges nothing: the platform compares the report
with what the race server will check. A path has to start with `content/` or `system/` and consist of plain
names, so the command cannot be used to look at anything but the game's content.

`start_bot_race` and `bot_race_result` drive a race against the game's own AI, offline: the first writes the
`race.ini` of a single-player race (the driver starts last behind bots in the same car) and starts the game,
the second says whether the game is still running and, once it has closed, how the race went, read from the
game's result file. The platform treats such a race as fun (XP and a best list), because only the driver's PC
sees it.

`setup_status` and `install_setup` are for the car setups of Assetto Corsa EVO. The first takes the places of
setups (the car's folder, the track's folder, the name of the file) and answers with the SHA-256 of each file,
or that it is not there; the platform knows whether that is the setup it hands out or one the driver changed.
The second takes one place and the content of the file and writes it there. A place consists of plain names
and the file ends in `.carsetup`, so nothing can be written outside the game's setup folder; the content is
at most 64 KB. Loading a setup stays with the driver, in the game's setup screen: the app cannot know which
setup the game drives with.

`update_check` and `update_install` keep the `setup.exe` install up to date. The bundled start page calls them
before the interface loads; since 0.8.0 the hosted interface may call them too, for a notice while the app
stays open and a "check for updates" button. The first asks the update manifest of this repository's GitHub releases (`latest.json`) for a newer
version, the second downloads that installer, checks its signature against the public key in
`tauri.conf.json` (tauri-plugin-updater, the signature cannot be skipped) and runs it; the installer closes
the app and starts the new version. An install the Store made, or a build started from the `target` folder,
answers that it does not update itself.

## Development

Prerequisites on Windows: Rust (MSVC toolchain), Visual Studio Build Tools with C++, Node.js.

```powershell
npm install
npm run dev          # shell against a platform on http://localhost:3000 (set DD_PLATFORM_URL to change)
npm run build        # release build with installer (needs TAURI_SIGNING_PRIVATE_KEY, see Releases)
cargo test           # all Rust tests; `cargo test -p dd-core` also runs on Linux and WSL
```

## Releases

A release is a tag `v<version>` with the version of `tauri.conf.json` (also in `Cargo.toml`, `package.json`
and the MSIX manifest). `.github/workflows/release.yml` builds the installer, signs the update with the
repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` and publishes `DigitalDrivers_<version>_x64-setup.exe`, its
`.sig` and `latest.json` (`scripts/release-manifest.mjs`) as a GitHub release; every installed app finds
that manifest under `releases/latest/download/latest.json`. The private key was made with
`npm run tauri signer generate -- -w ~/.tauri/dd-desktop.key -p <password>`; its public half is in
`tauri.conf.json`. A local release build needs the key too: `$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw
~/.tauri/dd-desktop.key` and `$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = <password>` (the key needs a
password: Windows cannot hold an empty environment variable, and without the variable the CLI prompts).
Lose the key and every installed app is stranded on its version, so keep it backed up outside this repository.

`scripts/update-check.sh` checks the self-update on this PC without GitHub: it serves a manifest that names
the freshly built installer as a newer version, starts the debug build against it
(`scripts/run-update-check.ps1`, `DD_UPDATE_URL`, development builds only) and watches the app download,
verify and install the release, which then starts and reports itself up to date.

## Packaging

```powershell
npm run tauri -- build --no-bundle      # release exe
scripts\pack-msix.ps1                   # unsigned MSIX: the form the Microsoft Store takes and signs itself
scripts\pack-msix.ps1 -DevCert          # MSIX signed with a generated development certificate, for local installs
```

To install the development package, trust the certificate once in an administrator PowerShell
(`winapp cert install dist\devcert.pfx`), then run `Add-AppxPackage dist\DigitalDrivers.msix`.
The manifest and tile images are in `packaging/msix/`.

## Automated end-to-end check

`scripts/run-shell-check.ps1` starts the debug build, waits, saves a screenshot of the app window and prints
the log. It is the automated end-to-end check that a hosted page can call the native commands.

`scripts/run-toast-check.ps1` starts the debug build with WebView2's remote debugging port, waits until the
platform is loaded and calls `show_toast` from that page (`scripts/cdp-eval.mjs` evaluates JavaScript in the
WebView, Node 22+). The log then shows the request with the app id that was used.

`scripts/run-join-check.ps1` checks the one-click join: it signs the page in as a driver (the platform's test
login, development only), opens an event and clicks "Join the race". It is started by
`dd-platform/scripts/real-game-check.sh --app`, which brings up the platform and a race server first and then
watches the real game connect.

`scripts/run-setups-check.ps1` opens the setups as a supporter, installs one and prints what the page says
before and after. Development builds take the game's folder from `DD_AC_EVO_USER_DIR` when it is set, so
`dd-platform/scripts/setups-check.sh` checks against a folder made up for it; release builds ignore the variable.

`scripts/run-scrutineering-check.ps1` runs scrutineering from an event page and prints the verdict. Development
builds take the game folder from `DD_ASSETTO_CORSA_DIR` when it is set, so
`dd-platform/scripts/scrutineering-check.sh` can check a copy with a changed `data.acd` without touching the
installed game; release builds ignore the variable.

## License

MIT, see [LICENSE](LICENSE).
