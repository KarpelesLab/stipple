# Post a key press and release to a window found by title (the window needn't
# be in the foreground), or with -Screenshot, save its frame to a PNG instead.
param([string]$Title, [int]$Key = 9, [string]$Screenshot = "")

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class PK {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
}
"@
[PK]::SetProcessDPIAware() | Out-Null
$hwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 40 -and $hwnd -eq [IntPtr]::Zero; $i++) {
  $hwnd = [PK]::FindWindowW([NullString]::Value, $Title)
  if ($hwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 250 }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "no window titled '$Title'" }
if ($Screenshot) {
  $r = New-Object RECT
  [PK]::DwmGetWindowAttribute($hwnd, 9, [ref]$r, 16) | Out-Null
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  $bmp = New-Object System.Drawing.Bitmap($w, $h)
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $gfx.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
  $bmp.Save((Join-Path $PWD $Screenshot))
  Write-Host "Saved $Screenshot (${w}x${h})"
} else {
  [PK]::SetForegroundWindow($hwnd) | Out-Null
  # WM_KEYDOWN then WM_KEYUP (repeat count 1; the release's transition bits).
  [PK]::PostMessageW($hwnd, 0x0100, [IntPtr]$Key, [IntPtr]1) | Out-Null
  [PK]::PostMessageW($hwnd, 0x0101, [IntPtr]$Key, [IntPtr]0xC0000001) | Out-Null
  Write-Host "Posted key $Key to '$Title'"
}
