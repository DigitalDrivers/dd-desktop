# Checks that a hosted page may show a native notification: starts the debug build with WebView2's
# remote debugging port, waits until the platform is loaded, calls the show_toast command from that
# page and prints the result and the shell's log. Needs the platform on $PlatformUrl and Node 22+.
param(
  [string]$PlatformUrl = 'http://localhost:3000',
  [int]$DebugPort = 9229,
  [int]$KeepAliveSeconds = 0
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_PLATFORM_URL = $PlatformUrl
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort"
$p = Start-Process -FilePath $exe -PassThru

try {
  # The start page hands over to the platform once its health check answers.
  $loaded = $false
  foreach ($i in 1..40) {
    Start-Sleep -Seconds 1
    try {
      $page = (Invoke-RestMethod "http://127.0.0.1:$DebugPort/json") | Where-Object { $_.type -eq 'page' } | Select-Object -First 1
      if ($page.url.StartsWith($PlatformUrl)) { $loaded = $true; break }
    } catch { }
  }
  "platform loaded in the shell: $loaded ($($page.url))"

  $call = "window.__TAURI__.core.invoke('show_toast', { title: 'Digital Drivers', body: 'Toast check: the server is open.' }).then(() => 'shown')"
  node (Join-Path $PSScriptRoot 'cdp-eval.mjs') $DebugPort $call
  if ($KeepAliveSeconds -gt 0) { Start-Sleep -Seconds $KeepAliveSeconds }
}
finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
