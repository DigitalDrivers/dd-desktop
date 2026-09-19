# Checks technical scrutineering for real, without starting the game: starts the debug build with WebView2's
# remote debugging port, signs the page in as the entered driver (the platform's test login, development only),
# opens the event, runs scrutineering and prints the verdict the page shows, the problems it lists and the
# shell's log. With -GameDir the debug build checks that folder instead of the installed game.
# Started for you by dd-platform/scripts/scrutineering-check.sh. Needs Node 22+.
param(
  [Parameter(Mandatory)][string]$EventId,
  [Parameter(Mandatory)][string]$SteamId,
  [Parameter(Mandatory)][string]$Name,
  [Parameter(Mandatory)][string]$LoginToken,
  [string]$GameDir = '',
  [string]$PlatformUrl = 'http://localhost:3000',
  [int]$DebugPort = 9229
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_PLATFORM_URL = $PlatformUrl
$env:DD_ASSETTO_CORSA_DIR = $GameDir
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
  # No double quotes inside the expressions: PowerShell drops them on the way to node.
  node $eval $DebugPort "fetch('/auth/test-login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-test-login-token': '$LoginToken' }, body: JSON.stringify({ steamId: '$SteamId', name: '$Name', role: 'driver' }) }).then(r => 'signed in: ' + r.status)"
  node $eval $DebugPort "(location.assign('/events/$EventId'), 'opening the event')"
  "event page loaded: $(Wait-ForPage "$PlatformUrl/events/$EventId")"

  $run = @"
(async () => {
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const find = id => document.querySelector('[data-testid=' + id + ']');
  for (let i = 0; i < 40 && !find('run-scrutineering'); i++) await wait(500);
  if (!find('run-scrutineering')) return 'no scrutineering button on the page: ' + (find('scrutineering-box')?.innerText ?? 'no scrutineering box');
  find('run-scrutineering').click();
  await wait(1500);
  for (let i = 0; i < 60 && find('run-scrutineering').disabled; i++) await wait(500);
  return { verdict: find('scrutineering-status').innerText.trim(), problems: [...(find('scrutineering-problems')?.querySelectorAll('li') ?? [])].map(li => li.innerText.trim()) };
})()
"@
  node $eval $DebugPort ($run -replace "`r?`n", ' ')
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
