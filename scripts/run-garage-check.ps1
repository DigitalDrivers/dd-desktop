# Checks the garage for real, without starting the game: starts the debug build against a platform, signs the
# page in as a driver (the platform's test login, development only), opens the garage of AC EVO and clicks
# "Into my garage" of one car, then prints the state of every car card before and after and the shell's log.
# The debug build takes the game's folder from -UserDir instead of Saved Games\ACE.
# Started for you by dd-platform/scripts/garage-check.sh. Needs Node 22+.
param(
  [Parameter(Mandatory)][string]$SteamId,
  [Parameter(Mandatory)][string]$LoginToken,
  [Parameter(Mandatory)][string]$UserDir,
  [Parameter(Mandatory)][string]$Car,
  [string]$PlatformUrl = 'http://localhost:3000',
  [int]$DebugPort = 9229
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_PLATFORM_URL = $PlatformUrl
$env:DD_AC_EVO_USER_DIR = $UserDir
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
  node $eval $DebugPort "fetch('/auth/test-login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-test-login-token': '$LoginToken' }, body: JSON.stringify({ steamId: '$SteamId', name: 'Garage Check', role: 'driver' }) }).then(r => 'signed in: ' + r.status)"
  node $eval $DebugPort "(location.assign('/evo/garage'), 'opening the garage')"
  "garage loaded: $(Wait-ForPage "$PlatformUrl/evo/garage")"
  # One line: no comments inside. It waits for the cards to know what the app has, clicks, waits for the end.
  $run = @"
(async () => {
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const states = () => Object.fromEntries([...document.querySelectorAll('[data-testid=garage] > li')].map(li => [li.querySelector('p').innerText.trim(), li.dataset.state]));
  for (let i = 0; i < 40 && Object.values(states()).every(s => s === 'browser'); i++) await wait(500);
  const before = states();
  const button = document.querySelector('[data-testid=install-$Car]');
  if (!button) return { before, problem: 'no install button for $Car' };
  button.click();
  await wait(1000);
  for (let i = 0; i < 240 && document.querySelector('[data-testid=install-$Car]') && !document.querySelector('[data-testid=garage-problem]'); i++) await wait(500);
  return { before, after: states(), problem: document.querySelector('[data-testid=garage-problem]')?.innerText ?? null };
})()
"@
  node $eval $DebugPort ($run -replace "`r?`n", ' ')
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
