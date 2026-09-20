# Checks a race against bots for real: starts the debug build, signs the page in as a driver (the platform's
# test login, development only), opens the event, clicks "Start race against bots", lets the game load and
# start the race, closes the game and prints what the page made of it. The game plays itself: nobody drives,
# so the result is whatever the game wrote when it closed.
# Started for you by dd-platform/scripts/bot-race-check.sh. Needs Node 22+.
param(
  [Parameter(Mandatory)][string]$EventId,
  [Parameter(Mandatory)][string]$SteamId,
  [Parameter(Mandatory)][string]$Name,
  [Parameter(Mandatory)][string]$LoginToken,
  [int]$RaceSeconds = 100,
  [string]$PlatformUrl = 'http://localhost:3000',
  [int]$DebugPort = 9229
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_PLATFORM_URL = $PlatformUrl
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

  $start = @"
(async () => {
  const wait = ms => new Promise(r => setTimeout(r, ms));
  for (let i = 0; i < 40 && !document.querySelector('[data-testid=start-bot-race]'); i++) await wait(500);
  const button = document.querySelector('[data-testid=start-bot-race]');
  if (!button) return 'no button: ' + (document.querySelector('[data-testid=bot-race]')?.innerText ?? 'no bot race box');
  button.click();
  await wait(2000);
  return document.querySelector('[data-testid=bot-race-state]')?.innerText ?? 'no state shown';
})()
"@
  node $eval $DebugPort ($start -replace "`r?`n", ' ')

  "the game is loading and racing for $RaceSeconds s"
  Start-Sleep -Seconds $RaceSeconds
  # Close the game the way a driver does; only then does it write its result.
  $game = Get-Process acs -ErrorAction SilentlyContinue
  if ($game) { $game | ForEach-Object { [void]$_.CloseMainWindow() }; $game | Wait-Process -Timeout 30 -ErrorAction SilentlyContinue }
  Get-Process acs -ErrorAction SilentlyContinue | Stop-Process -Force

  $read = @"
(async () => {
  const wait = ms => new Promise(r => setTimeout(r, ms));
  for (let i = 0; i < 30 && !document.querySelector('[data-testid=bot-race-result]'); i++) await wait(1000);
  return { result: document.querySelector('[data-testid=bot-race-result]')?.innerText ?? 'none', state: document.querySelector('[data-testid=bot-race-state]')?.innerText ?? 'none' };
})()
"@
  node $eval $DebugPort ($read -replace "`r?`n", ' ')
}
finally {
  Get-Process acs -ErrorAction SilentlyContinue | Stop-Process -Force
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
