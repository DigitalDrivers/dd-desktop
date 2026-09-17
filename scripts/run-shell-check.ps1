# Starts the debug build of the shell against a platform URL, waits, optionally saves a screenshot of the
# app window (only that window's area), stops the app and prints its log file.
# Used for automated end-to-end checks: the log shows which page called the native commands.
param(
  [string]$PlatformUrl = 'http://localhost:3000',
  [int]$Seconds = 20,
  [string]$Screenshot = ''
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\dd-desktop.exe'
$log = Join-Path $env:TEMP 'dd-desktop.log'
Remove-Item $log -ErrorAction SilentlyContinue

$env:DD_PLATFORM_URL = $PlatformUrl
$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds $Seconds
$p.Refresh()
"alive after $Seconds s: $(-not $p.HasExited)"

if ($Screenshot -and -not $p.HasExited -and $p.MainWindowHandle -ne 0) {
  Add-Type -AssemblyName System.Drawing
  Add-Type @"
using System;
using System.Runtime.InteropServices;
public class DdWin {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
  [DdWin]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
  Start-Sleep -Milliseconds 700
  $r = New-Object DdWin+RECT
  [DdWin]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null
  $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
  $bmp.Save($Screenshot)
  "screenshot: $Screenshot"
}

if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }

'--- log:'
if (Test-Path $log) { Get-Content $log } else { '(no log file written)' }
