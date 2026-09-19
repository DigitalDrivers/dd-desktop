# dd-desktop

Digital Drivers desktop app for Windows. Joins races with one click, runs technical scrutineering on cars
and tracks, installs content. Open-source shell built with Tauri.

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
- It writes a log to `%TEMP%\dd-desktop.log`.

Nothing else is read, and nothing is uploaded by the shell itself.

## How it works

| Part | What |
| --- | --- |
| `src/` | Bundled start page: checks that the platform is reachable, then opens it. Offline it shows a system check. |
| `src-tauri/` | The Tauri shell with the native commands `platform_url`, `system_check`, `show_toast`, `join_race` and `scrutineer`. |
| `src-tauri/capabilities/` | Which page may call which command. The hosted interface only gets the commands listed in `platform.json`; `dev-localhost.json` is enabled for development builds only. |
| `crates/dd-core/` | Platform-independent logic (locating Assetto Corsa, the join ticket and the `race.ini` of an online session, hashing game files for scrutineering), unit-tested on any machine. |

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

## Development

Prerequisites on Windows: Rust (MSVC toolchain), Visual Studio Build Tools with C++, Node.js.

```powershell
npm install
npm run dev          # shell against a platform on http://localhost:3000 (set DD_PLATFORM_URL to change)
npm run build        # release build with installer
cargo test           # all Rust tests; `cargo test -p dd-core` also runs on Linux and WSL
```

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

`scripts/run-scrutineering-check.ps1` runs scrutineering from an event page and prints the verdict. Development
builds take the game folder from `DD_ASSETTO_CORSA_DIR` when it is set, so
`dd-platform/scripts/scrutineering-check.sh` can check a copy with a changed `data.acd` without touching the
installed game; release builds ignore the variable.

## License

MIT, see [LICENSE](LICENSE).
