# Checks the car setups for AC EVO for real, without starting the game: starts the debug build with WebView2's
# remote debugging port, signs the page in as a supporter (the platform's test login, development only), opens
# the setups and prints what the page says about the first car before and after a click, and the shell's log.
# -Action install clicks "Install setup" of the Hotlap setup, restore asks for the original of a changed one,
# look clicks nothing. With -UserDir the debug build takes that folder for the game's instead of Saved Games\ACE.
# Started for you by dd-platform/scripts/setups-check.sh. Needs Node 22+.
param(
  [Parameter(Mandatory)][string]$SteamId,
  [Parameter(Mandatory)][string]$Name,
  [Parameter(Mandatory)][string]$LoginToken,
  [string]$UserDir = '',
  [ValidateSet('look', 'install', 'restore')][string]$Action = 'look',
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
  node $eval $DebugPort "fetch('/auth/test-login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-test-login-token': '$LoginToken' }, body: JSON.stringify({ steamId: '$SteamId', name: '$Name', role: 'driver' }) }).then(r => 'signed in: ' + r.status)"
  # The pages of AC EVO are for drivers who chose it.
  node $eval $DebugPort "fetch('/api/me/game', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ game: 'acevo' }) }).then(r => 'chose AC EVO: ' + r.status)"
  node $eval $DebugPort "(location.assign('/evo/setups'), 'opening the setups')"
  "setups loaded: $(Wait-ForPage "$PlatformUrl/evo/setups")"

  # The script becomes one line: no comments in it. The page is there before the app has answered: until then
  # it shows what a browser gets, so the script waits for the rows of the app and takes their absence as the
  # answer only after 20 seconds. The original of a changed setup takes a second click (what the driver saved
  # is lost).
  $run = @"
(async () => {
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const find = (id, from = document) => from.querySelector('[data-testid=' + id + ']');
  for (let i = 0; i < 40 && !find('setup-install'); i++) await wait(500);
  const hint = find('setup-app-hint')?.innerText.trim() ?? null;
  const rows = () => [...(find('setup-install')?.querySelectorAll('li') ?? [])];
  const states = () => rows().map(li => li.dataset.state);
  const buttons = () => [...(rows()[0]?.querySelectorAll('button') ?? [])];
  if (!rows().length) return { hint, problem: find('setup-problem')?.innerText.trim() ?? null };
  await wait(1500);
  const car = find('setups').querySelector('li p').innerText.trim();
  const before = states();
  const action = '$Action';
  if (action !== 'look') {
    if (!buttons().length) return { car, before, problem: 'no button in the row of the Hotlap setup' };
    buttons()[0].click();
    await wait(500);
    if (action === 'restore') buttons()[0].click();
    for (let i = 0; i < 40 && states()[0] === before[0] && !find('setup-problem'); i++) await wait(500);
  }
  return { car, hint, before, after: states(), problem: find('setup-problem')?.innerText.trim() ?? null };
})()
"@
  node $eval $DebugPort ($run -replace "`r?`n", ' ')
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
