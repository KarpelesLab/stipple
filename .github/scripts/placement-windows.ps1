# App::remember_window on Windows, with the lifecycle example: moved and
# resized, the window opens there on the next run; maximized, it opens
# maximized; a placement on a monitor that's gone opens on one that is
# there. The placement file is read between runs.
param(
  [string]$Exe = ".\target\release\examples\lifecycle.exe",
  [string]$Out = "visual_output"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$file = Join-Path $env:APPDATA "com.karpeleslab.stipple.lifecycle\windows\main.placement"
Remove-Item -Force -ErrorAction SilentlyContinue $file

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class P {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
}
"@

function Open-App($n) {
  $p = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput (Join-Path $Out "placement-$n.log") `
    -RedirectStandardError (Join-Path $Out "placement-$n.err")
  Start-Sleep -Seconds 4
  $h = [P]::FindWindowW("StippleWindowClass", "Stipple lifecycle")
  if ($h -eq [IntPtr]::Zero) { throw "run ${n}: no window" }
  return @($p, $h)
}
function Close-App($p) { Stop-Process -Id $p.Id -Force; Start-Sleep -Seconds 1 }
function Rect($h) { $r = New-Object P+RECT; [P]::GetWindowRect($h, [ref]$r) | Out-Null; return $r }

# Run 1: move, then resize, a second and a half apart (writes are at most
# once a second while the window changes).
$p, $h = Open-App 1
[P]::SetWindowPos($h, [IntPtr]::Zero, 150, 120, 0, 0, 0x0015) | Out-Null  # NOSIZE|NOZORDER|NOACTIVATE
Start-Sleep -Milliseconds 1500
[P]::SetWindowPos($h, [IntPtr]::Zero, 0, 0, 640, 500, 0x0016) | Out-Null  # NOMOVE|NOZORDER|NOACTIVATE
Start-Sleep -Milliseconds 1500
$r = Rect $h
Write-Host "run 1: $($r.Left),$($r.Top) $($r.Right - $r.Left)x$($r.Bottom - $r.Top)"
Close-App $p
Write-Host "saved:"; Get-Content $file
$text = Get-Content $file -Raw
if ($text -notmatch "(?m)^x=150$" -or $text -notmatch "(?m)^y=120$") { throw "the position wasn't saved" }

# Run 2: back where it was left; then maximize it.
$p, $h = Open-App 2
$r2 = Rect $h
Write-Host "run 2: $($r2.Left),$($r2.Top) $($r2.Right - $r2.Left)x$($r2.Bottom - $r2.Top)"
if ($r2.Left -ne 150 -or $r2.Top -ne 120) { throw "run 2 didn't open where run 1 was left" }
if (($r2.Right - $r2.Left) -ne ($r.Right - $r.Left) -or ($r2.Bottom - $r2.Top) -ne ($r.Bottom - $r.Top)) {
  throw "run 2 didn't open at run 1's size"
}
[P]::ShowWindow($h, 3) | Out-Null  # SW_MAXIMIZE
Start-Sleep -Milliseconds 1500
Close-App $p
Get-Content $file
$text = Get-Content $file -Raw
if ($text -notmatch "(?m)^maximized=true$" -or $text -notmatch "(?m)^x=150$") { throw "maximized, the restored bounds weren't kept" }

# Run 3: opens maximized.
$p, $h = Open-App 3
$zoomed = [P]::IsZoomed($h)
Write-Host "run 3: maximized=$zoomed"
Close-App $p
if (-not $zoomed) { throw "run 3 didn't open maximized" }

# Run 4: a monitor that's gone.
Set-Content -Path $file -Value "x=30000`ny=20000`nwidth=500`nheight=400`nmaximized=false" -NoNewline
$p, $h = Open-App 4
$r4 = Rect $h
$monitor = [P]::MonitorFromWindow($h, 0)  # MONITOR_DEFAULTTONULL
Write-Host "run 4: $($r4.Left),$($r4.Top) on monitor $monitor"
Close-App $p
if ($monitor -eq [IntPtr]::Zero) { throw "run 4 opened off every monitor" }
Write-Host "placement ok"
