# Drive the lifecycle example's close and session-end handling on Windows:
# WM_QUERYENDSESSION (logging off) is refused while work runs and asks the
# app, whose close hook cancels and shows its banner; WM_CLOSE then confirms,
# and the app closes its window itself once its check is done.
param(
  [string]$Exe = ".\target\release\lifecycle.exe",
  [string]$Out = "visual_output"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$log = Join-Path $Out "lifecycle-windows.log"
$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
  -RedirectStandardError (Join-Path $Out "lifecycle-windows.err")
Start-Sleep -Seconds 5

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class L {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
}
"@
$WM_CLOSE = 0x0010
$WM_QUERYENDSESSION = 0x0011

$hwnd = [L]::FindWindowW("StippleWindowClass", "Stipple lifecycle")
if ($hwnd -eq [IntPtr]::Zero) { throw "no lifecycle window" }

# Logging off: refused (0) while the app has a close hook to ask.
$answer = [L]::SendMessageW($hwnd, $WM_QUERYENDSESSION, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Host "WM_QUERYENDSESSION answered $answer"
if ($answer -ne [IntPtr]::Zero) { throw "the session end was not refused" }
Start-Sleep -Seconds 2
if ($proc.HasExited) { throw "the app quit despite cancelling" }

# Close: the app confirms, checks, then closes the window itself.
[L]::PostMessageW($hwnd, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
if (-not $proc.WaitForExit(15000)) { $proc.Kill(); throw "the app didn't close" }

$text = Get-Content $log -Raw
Write-Host "=== lifecycle log ==="; Write-Host $text
foreach ($want in @("close vetoed", "close confirmed, checking", "check done, closing", "event loop exited")) {
  if ($text -notmatch [regex]::Escape("lifecycle: $want")) { throw "missing: $want" }
}
