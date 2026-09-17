# Packs the release build of the shell as an MSIX with the winapp CLI.
#   -DevCert  signs the package with a generated development certificate so it can be installed locally
#             (the certificate has to be trusted once: run `winapp cert install dist\devcert.pfx` as administrator).
# Without -DevCert the package stays unsigned: that is the form the Microsoft Store takes, it signs the package itself.
param([switch]$DevCert)
$ErrorActionPreference = 'Stop'
$env:WINAPP_CLI_TELEMETRY_OPTOUT = '1'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$exe = 'target\release\dd-desktop.exe'
if (-not (Test-Path $exe)) { throw "Build the release first: npm run tauri -- build --no-bundle" }

$stage = 'dist\msix-layout'
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage

$out = 'dist\DigitalDrivers.msix'
Remove-Item $out -ErrorAction SilentlyContinue
$packArgs = @('pack', $stage, '--manifest', 'packaging\msix\Package.appxmanifest', '--output', $out, '--exe', 'dd-desktop.exe')
if ($DevCert) {
  if (-not (Test-Path 'dist\devcert.pfx')) {
    winapp cert generate --publisher 'CN=Digital Drivers Dev' --output 'dist\devcert.pfx'
  }
  $packArgs += @('--cert', 'dist\devcert.pfx')
}
winapp @packArgs
if ($LASTEXITCODE -ne 0) { throw "winapp pack failed with exit code $LASTEXITCODE" }

Get-Item $out | Select-Object Name, Length
