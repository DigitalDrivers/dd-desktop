# Checks the one-click join for real: starts the debug build with WebView2's remote debugging port, signs the
# page in as the driver (the platform's test login, development only), opens the event and clicks
# "Join the race". Prints what the page answered and the shell's log. The game keeps running when the
# script ends: whoever called it watches the race server and closes the game.
# Needs the platform on $PlatformUrl with a running race server for the event, Steam signed in, Node 22+.
# Started for you by dd-platform/scripts/real-game-check.sh --app.
param(
  [Parameter(Mandatory)][string]$EventId,
  [Parameter(Mandatory)][string]$SteamId,
  [Parameter(Mandatory)][string]$Name,
  [Parameter(Mandatory)][string]$LoginToken,
  [string]$Role = 'driver',
  [string]$Button = 'join-race',
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
  node $eval $DebugPort "fetch('/auth/test-login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-test-login-token': '$LoginToken' }, body: JSON.stringify({ steamId: '$SteamId', name: '$Name', role: '$Role' }) }).then(r => 'signed in: ' + r.status)"
  node $eval $DebugPort "(location.assign('/events/$EventId'), 'opening the event')"
  "event page loaded: $(Wait-ForPage "$PlatformUrl/events/$EventId")"

  # The button appears once the page knows it runs inside the app. Click it and read what the page says.
  $click = @"
(async () => {
  for (let i = 0; i < 40 && !document.querySelector('[data-testid=$Button]'); i++) await new Promise(r => setTimeout(r, 500));
  const button = document.querySelector('[data-testid=$Button]');
  if (!button) return 'no join button on the page: ' + (document.querySelector('[data-testid=server-box]')?.innerText ?? 'no server box');
  button.click();
  for (let i = 0; i < 40 && !document.querySelector('[data-testid=join-message]'); i++) await new Promise(r => setTimeout(r, 500));
  return 'clicked [' + button.innerText.trim() + ']: ' + (document.querySelector('[data-testid=join-message]')?.innerText ?? 'no answer');
})()
"@
  node $eval $DebugPort ($click -replace "`r?`n", ' ')
  Start-Sleep -Seconds 2
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
