# Checks the self-update for real: starts the debug build with an update manifest of its own (-ManifestUrl,
# served by scripts/update-check.sh), waits until the app has found the release, downloaded it and handed
# over to the installer, which closes the app, and prints the shell's log. The installer then puts the
# release build on this PC and starts it. Started for you by scripts/update-check.sh.
param(
  [Parameter(Mandatory)][string]$ManifestUrl
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_UPDATE_URL = $ManifestUrl
$p = Start-Process -FilePath $exe -PassThru
try {
  foreach ($i in 1..60) {
    Start-Sleep -Seconds 1
    if ((Test-Path $log) -and (Select-String -Path $log -Pattern 'update_install -> downloaded|update_install failed|update_check failed|update_check -> up to date' -Quiet)) { break }
  }
  # The installer closes the app once the signature is good.
  foreach ($i in 1..20) { if ($p.HasExited) { break }; Start-Sleep -Seconds 1 }
  "the app closed by itself: $($p.HasExited)"
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
