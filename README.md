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
| `src-tauri/` | The Tauri shell with the native commands `platform_url` and `system_check`. |
| `src-tauri/capabilities/` | Which page may call which command. The hosted interface only gets the commands listed in `platform.json`; `dev-localhost.json` is enabled for development builds only. |
| `crates/dd-core/` | Platform-independent logic (locating Assetto Corsa), unit-tested on any machine. |

Commands must be declared in `src-tauri/build.rs` and allowed per origin in a capability file. A hosted page
cannot call anything that is not listed for its origin.

## Development

Prerequisites on Windows: Rust (MSVC toolchain), Visual Studio Build Tools with C++, Node.js.

```powershell
npm install
npm run dev          # shell against a platform on http://localhost:3000 (set DD_PLATFORM_URL to change)
npm run build        # release build with installer
cargo test           # all Rust tests; `cargo test -p dd-core` also runs on Linux and WSL
```

`scripts/run-shell-check.ps1` starts the debug build, waits, saves a screenshot of the app window and prints
the log. It is the automated end-to-end check that a hosted page can call the native commands.

## License

MIT, see [LICENSE](LICENSE).
