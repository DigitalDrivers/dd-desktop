# Checks the update notice while the app runs: starts the debug build against an update manifest of its own
# (-ManifestUrl, served by dd-platform/scripts/update-notice-check.sh, which answers "up to date" at start
# and names a newer version afterwards), waits for the hosted interface, clicks "Check for updates" on the home
# page and prints what the page shows: the answer next to the button and the notice at the top. Nothing is
# installed. Started for you by dd-platform/scripts/update-notice-check.sh. Needs Node 22+.
param(
  [Parameter(Mandatory)][string]$ManifestUrl,
  [string]$PlatformUrl = 'http://localhost:3000',
  [int]$DebugPort = 9229
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_PLATFORM_URL = $PlatformUrl
$env:DD_UPDATE_URL = $ManifestUrl
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort"
$p = Start-Process -FilePath $exe -PassThru
$eval = Join-Path $PSScriptRoot 'cdp-eval.mjs'

function Wait-ForPage([string]$prefix) {
  foreach ($i in 1..40) {
    Start-Sleep -Seconds 1
    try {
      $page = (Invoke-RestMethod "http://127.0.0.1:$DebugPort/json") | Where-Object { $_.type -eq 'page' } | Select-Object -First 1
      if ($page.url.StartsWith($prefix)) { return $true }
    } catch { }
  }
  return $false
}

try {
  "platform loaded in the shell: $(Wait-ForPage $PlatformUrl)"
  # The script becomes one line: no comments in it. It waits for the button, clicks it and reads the answer.
  $run = @"
(async () => {
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const find = id => document.querySelector('[data-testid=' + id + ']');
  for (let i = 0; i < 40 && !find('check-update'); i++) await wait(500);
  if (!find('check-update')) return { problem: 'no button on the page' };
  const noticeBefore = !!find('desktop-update');
  find('check-update').click();
  for (let i = 0; i < 40 && !find('update-result'); i++) await wait(500);
  return { noticeBefore, result: find('update-result')?.innerText.trim() ?? null, notice: find('desktop-update')?.innerText.trim() ?? null };
})()
"@
  node $eval $DebugPort ($run -replace "`r?`n", ' ')
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
