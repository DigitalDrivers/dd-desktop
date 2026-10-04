# dd-desktop

Digital Drivers desktop app for Windows, for Assetto Corsa EVO: puts the club's cars and car setups into the
game and keeps them up to date, starts the game for a server join, and updates itself. Open-source shell built
with Tauri.

Until 2026-10-03 it also served Assetto Corsa (one-click race joins, technical scrutineering, races against
bots, Content Manager checks); the git tag `ac-final` is the last state with that.

The app is a thin shell: it loads the hosted Digital Drivers interface and adds a small set of native
commands. It contains no business logic; decisions are made on the server. The source is public so every
driver can check what the app does on their PC.

## What the app touches on your PC

- It reads Steam's install path from the registry and Steam's `libraryfolders.vdf` to find Assetto Corsa EVO,
  and which account is signed in to Steam (`ActiveProcess\ActiveUser` in Steam's registry key).
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
- Only if you switch on the live map on the platform (Telemetry and live map): while AC EVO drives an online
  session, the page asks the app once a second for what the game publishes in its shared memory
  (`Local\acevo_pmf_graphics` and `Local\acevo_pmf_static`, documented by Kunos; the app only opens them for
  reading): the track, and the ids and positions of the cars your game sees. The page sends that to the
  platform, which shows the cars of our servers on the live map. Nothing of it is read while the switch is off.
- Only if you switch on lap summaries on the platform (Telemetry): once the page first asks for laps, the app
  reads `Local\acevo_pmf_physics`, `Local\acevo_pmf_graphics` and `Local\acevo_pmf_static` ten times a second
  until it is closed, while you drive live. When a lap ends, it reads the game's newest log
  (`Saved Games\ACE\Logs\log-*.txt`) to see which car and preset you selected last, because the shared memory
  names the car only by its display name. Only a lap of one of the club's cars (ids starting with `dd_`) is kept;
  the laps of any other car are dropped right there. Per lap it keeps a summary: car, track, lap time, valid,
  pit, fuel used, air and road temperature, top speed and the gear it was reached in, highest rpm, per wheel tyre
  pressure and core temperature (average, maximum), inner, middle and outer tyre temperature (average), brake
  temperature (maximum), suspension travel (maximum, average) and how often the wheel locked under braking or
  spun under throttle, the ride height (minimum, average), the brake bias and the front's share of the brake
  torque, the balance in corners (front against rear slip angle), the highest lateral and braking G, and the
  preset (the car's stage or variant). Time in the pit lane is left out. The page fetches the summaries and
  sends them to the platform; nothing else of the drive or of the log is kept.
- When you join one of our servers, the app starts Assetto Corsa EVO through Steam (`steam://run/3058630`); the
  page has put the server's join string on the clipboard, which the game's server list takes.
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
| `src-tauri/` | The Tauri shell with the native commands `platform_url`, `system_check`, `show_toast`, `setup_status`, `install_setup`, `update_check`, `update_install`, `launch_ac_evo`, `car_status`, `install_car`, `live_snapshot` and `take_laps`. |
| `src-tauri/capabilities/` | Which page may call which command. The hosted interface only gets the commands listed in `platform.json`; `dev-localhost.json` is enabled for development builds only. |
| `crates/dd-core/` | Platform-independent logic (locating Assetto Corsa EVO and the Steam account, car setups, the club's car packages and their saved cars), unit-tested on any machine. |

Commands must be declared in `src-tauri/build.rs` and allowed per origin in a capability file. A hosted page
cannot call anything that is not listed for its origin.

`show_toast` shows a Windows notification (plain title and text, cut to a sane length). The hosted interface
calls it when a car or a setup of the club arrived on the PC, because a WebView has no push service. Windows drops
toasts of an app id it does not know, so the shell picks the id by how it runs: the package's own id when
installed from the Store or as MSIX, the Tauri identifier for an installed build, and PowerShell's id for a
build started from the `target` folder.

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

`scripts/run-setups-check.ps1` opens the setups as a supporter, installs one and prints what the page says
before and after. Development builds take the game's folder from `DD_AC_EVO_USER_DIR` when it is set, so
`dd-platform/scripts/setups-check.sh` checks against a folder made up for it; release builds ignore the variable.

`scripts/run-garage-check.ps1` installs one of the club's cars from the garage into a folder made up for the
check and prints what the page says (`dd-platform/scripts/garage-check.sh`), and
`scripts/run-update-notice-check.ps1` clicks "Check for updates" (`dd-platform/scripts/update-notice-check.sh`
and `content-update-check.sh`).

## License

MIT, see [LICENSE](LICENSE).
