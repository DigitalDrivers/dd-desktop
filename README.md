# dd-desktop

Digital Drivers desktop app for Windows. Joins races with one click, runs technical scrutineering on cars
and tracks, installs content. Open-source shell built with Tauri.

The app is a thin shell: it loads the hosted Digital Drivers interface and adds a small set of native
commands. It contains no business logic; decisions are made on the server. The source is public so every
driver can check what the app reads on their PC: checksums of files inside the Assetto Corsa folder, nothing else.

## Status

| Part | State |
| --- | --- |
| `crates/dd-core` | Platform-independent logic, unit-tested. Today: locating Assetto Corsa in the Steam libraries. |
| Tauri shell (`src-tauri/`) | Not started. Needs the Windows toolchain (Rust MSVC, Visual Studio Build Tools, winapp CLI). |

## Development

```bash
cargo test            # runs on Windows, Linux and WSL
```

The Tauri shell is built on Windows. Logic that does not need Windows APIs belongs in `crates/dd-core`
so it can be tested everywhere.

## License

MIT, see [LICENSE](LICENSE).
