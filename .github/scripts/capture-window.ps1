# Launch a Stipple example, screenshot just its window, and stop it.
# -Arguments are passed to the example; -Title is its window title.
param([string]$Exe, [string]$Out, [string]$Title = "Stipple", [string]$Arguments = "")

New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
if ($Arguments) {
  $proc = Start-Process -FilePath $Exe -ArgumentList $Arguments -PassThru
} else {
  $proc = Start-Process -FilePath $Exe -PassThru
}
Start-Sleep -Seconds 5
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class Win {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  // The extended frame bounds exclude the invisible resize borders.
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
}
"@
[Win]::SetProcessDPIAware() | Out-Null
# PowerShell turns $null into "" for a string parameter; this stays a real null.
$hwnd = [Win]::FindWindowW([NullString]::Value, $Title)
if ($hwnd -eq [IntPtr]::Zero) { throw "no window titled '$Title'" }
[Win]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Seconds 1
$r = New-Object RECT
[Win]::DwmGetWindowAttribute($hwnd, 9, [ref]$r, 16) | Out-Null
$w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
$bmp.Save((Join-Path $PWD $Out))
Write-Host "Saved $Out (${w}x${h})"
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
