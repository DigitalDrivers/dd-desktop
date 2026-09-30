#!/usr/bin/env bash
# The self-update from end to end, on this PC: serves an update manifest that names the freshly built and
# signed installer as a newer version, starts the debug build against it and watches the app find, download,
# verify and install the release; then the installed app has to start and report itself up to date against
# the real manifest on GitHub. Leaves the release build installed for the current user.
#
#   scripts/update-check.sh
#
# Needs: WSL2 on a Windows PC, the debug build (npm run tauri -- build --debug --no-bundle) and the signed
# release build (npm run tauri -- build with TAURI_SIGNING_PRIVATE_KEY set) of the same version, port 8765 free.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
VERSION="$(python3 -c "import json; print(json.load(open('$REPO/src-tauri/tauri.conf.json'))['version'])")"
BUILT="$REPO/target/release/bundle/nsis/Digital Drivers_${VERSION}_x64-setup.exe"
[ -f "$REPO/target/debug/dd-desktop.exe" ] || { echo "the debug build is missing" >&2; exit 1; }
[ -f "$BUILT.sig" ] || { echo "the signed release build is missing ($BUILT.sig)" >&2; exit 1; }
if (exec 3<>/dev/tcp/127.0.0.1/8765) 2>/dev/null; then echo "port 8765 is in use" >&2; exit 1; fi
WORK="$(mktemp -d -t dd-update-XXXXXX)"
ps() { powershell.exe -NoProfile -Command "$1" 2>/dev/null | tr -d '\r'; }
cleanup() { [ -n "${SERVER:-}" ] && kill "$SERVER" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

# The manifest claims a version above the built one, so the app takes the built installer as the update.
cp "$BUILT" "$WORK/DigitalDrivers_${VERSION}_x64-setup.exe"
python3 - "$WORK" "$VERSION" "$BUILT.sig" <<'PY'
import json, sys
work, version, sig = sys.argv[1:]
major, minor, patch = version.split(".")
claimed = "%s.%s.%d" % (major, minor, int(patch) + 1)
json.dump({"version": claimed, "platforms": {"windows-x86_64": {"signature": open(sig).read().strip(),
           "url": "http://127.0.0.1:8765/DigitalDrivers_%s_x64-setup.exe" % version}}}, open(work + "/latest.json", "w"))
print("manifest claims %s for the installer of %s" % (claimed, version))
PY
(cd "$WORK" && exec python3 -m http.server 8765 --bind 0.0.0.0 > "$WORK/http.log" 2>&1) & SERVER=$!
sleep 1

echo "=== 1. the debug build updates itself from the manifest"
OUT=$(cd "$REPO" && powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/run-update-check.ps1 -ManifestUrl http://127.0.0.1:8765/latest.json 2>&1 | tr -d '\r')
echo "$OUT" | grep -E "update_|closed by itself"
failed=0
echo "$OUT" | grep -q "update_install -> downloaded" || { echo "FAILED: the app did not download the release"; failed=1; }
echo "$OUT" | grep -q "update_install failed" && { echo "FAILED: the app did not accept the release"; failed=1; }
echo "$OUT" | grep -q "the app closed by itself: True" || { echo "FAILED: the app did not hand over to the installer"; failed=1; }
grep -q "DigitalDrivers_${VERSION}_x64-setup.exe" "$WORK/http.log" || { echo "FAILED: the installer was not downloaded"; failed=1; }

echo "=== 2. the installed release build starts and checks the real manifest"
# The installer starts the new version itself once it is done; its log lines follow the debug build's.
LOG="$(wslpath "$(ps '$env:TEMP')")/dd-desktop.log"
for _ in $(seq 1 60); do grep -q "update_check -> \(up to date\|.* is out\)\|update_check failed" "$LOG" 2>/dev/null && grep -c "platform_url" "$LOG" | grep -q "^2" && break; sleep 2; done
grep -E "platform_url|update_check" "$LOG" | tail -3
INSTALLED="$(ps "(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Digital Drivers' -ErrorAction SilentlyContinue).DisplayVersion")"
echo "installed for the current user: ${INSTALLED:-nothing}"
[ "$INSTALLED" = "$VERSION" ] || { echo "FAILED: version $VERSION is not installed"; failed=1; }
ps "Get-Process dd-desktop -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Path" | grep -v "target" | grep -q "dd-desktop.exe" || { echo "FAILED: the installed app is not running"; failed=1; }

[ $failed = 0 ] && echo "== update check passed"
exit $failed
