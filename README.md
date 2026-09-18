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
- It writes a log to `%TEMP%\dd-desktop.log`.

Nothing else is read, and nothing is uploaded by the shell itself.

## How it works

| Part | What |
| --- | --- |
| `src/` | Bundled start page: checks that the platform is reachable, then opens it. Offline it shows a system check. |
| `src-tauri/` | The Tauri shell with the native commands `platform_url`, `system_check` and `show_toast`. |
| `src-tauri/capabilities/` | Which page may call which command. The hosted interface only gets the commands listed in `platform.json`; `dev-localhost.json` is enabled for development builds only. |
| `crates/dd-core/` | Platform-independent logic (locating Assetto Corsa), unit-tested on any machine. |

Commands must be declared in `src-tauri/build.rs` and allowed per origin in a capability file. A hosted page
cannot call anything that is not listed for its origin.

`show_toast` shows a Windows notification (plain title and text, cut to a sane length). The hosted interface
calls it for new notifications of the signed-in driver, because a WebView has no push service. Windows drops
toasts of an app id it does not know, so the shell picks the id by how it runs: the package's own id when
installed from the Store or as MSIX, the Tauri identifier for an installed build, and PowerShell's id for a
build started from the `target` folder.

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

## License

MIT, see [LICENSE](LICENSE).
