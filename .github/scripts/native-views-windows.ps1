# The native theme's live controls on Windows (see docs/native-theme.md, "Real
# controls"): run nativegallery's self-test (NATIVEGALLERY_SELFTEST), which
# reads its child-window controls back and acts on them; meanwhile click Save
# for real (SendInput, at the BUTTON child window's middle) and check the app
# counted it, screenshot the window with the self-test's panel over the
# controls and after, and walk the window through UI Automation to check
# each live control is one element (the BUTTON's), not two.
param(
  [string]$Exe = ".\target\release\examples\nativegallery.exe",
  [string]$Out = "visual_output"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$log = Join-Path $Out "native-views.log"
$env:NATIVEGALLERY_SELFTEST = "1"
$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
  -RedirectStandardError (Join-Path $Out "native-views.err")
Remove-Item Env:NATIVEGALLERY_SELFTEST

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class NV {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, string cls, string title);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
}
"@
[NV]::SetProcessDPIAware() | Out-Null

function Wait-Log([string]$pattern, [int]$seconds = 60) {
  for ($i = 0; $i -lt $seconds * 4; $i++) {
    if ((Test-Path $log) -and (Select-String -Path $log -Pattern $pattern -Quiet)) { return $true }
    Start-Sleep -Milliseconds 250
  }
  return $false
}

function Shot([IntPtr]$hwnd, [string]$file) {
  $r = New-Object RECT
  [NV]::DwmGetWindowAttribute($hwnd, 9, [ref]$r, 16) | Out-Null
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  $bmp = New-Object System.Drawing.Bitmap($w, $h)
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $gfx.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
  $bmp.Save((Join-Path (Get-Location) (Join-Path $Out $file)))
  Write-Host "Saved $file (${w}x${h})"
}

$failures = @()
try {
  if (-not (Wait-Log 'nativegallery: live' 60)) { throw "no live controls logged" }
  $hwnd = [NV]::FindWindowW([NullString]::Value, "Stipple")
  if ($hwnd -eq [IntPtr]::Zero) { throw "no window titled Stipple" }
  [NV]::SetForegroundWindow($hwnd) | Out-Null

  # The controls are child windows of the app's.
  $save = [NV]::FindWindowExW($hwnd, [IntPtr]::Zero, "Button", "Save")
  Write-Host "Save BUTTON child window: $save"
  if ($save -eq [IntPtr]::Zero) { $failures += "no BUTTON child window titled Save" }
  $track = [NV]::FindWindowExW($hwnd, [IntPtr]::Zero, "msctls_trackbar32", [NullString]::Value)
  Write-Host "trackbar child window: $track"
  if ($track -eq [IntPtr]::Zero) { $failures += "no trackbar child window" }

  # The self-test's panel over the controls (they're pictures under it).
  if (Wait-Log 'panel up' 30) {
    Shot $hwnd "windows-native-panel.png"
  } else { $failures += "the panel never came up" }
  if (-not (Wait-Log 'selftest:' 30)) { $failures += "the self-test never finished" }
  Start-Sleep -Seconds 1

  # Screen readers: each live control once, as the child window's element.
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,
    [System.Windows.Automation.Condition]::TrueCondition)
  $saves = 0
  $saveRect = $null
  foreach ($e in $all) {
    $c = $e.Current
    Write-Host ("uia: {0} {1} '{2}' class={3} hwnd={4}" -f $c.ControlType.ProgrammaticName, $c.LocalizedControlType, $c.Name, $c.ClassName, $c.NativeWindowHandle)
    if ($c.Name -eq "Save") { $saves++; $saveRect = $c.BoundingRectangle }
  }
  if ($saves -ne 1) { $failures += "UIA has $saves elements named Save, not 1" }

  # A real click on Save, where UIA says its BUTTON is on screen.
  if ($saveRect -ne $null) {
    $x = [int]($saveRect.X + $saveRect.Width / 2); $y = [int]($saveRect.Y + $saveRect.Height / 2)
    Write-Host "clicking Save at $x,$y"
    [NV]::SetForegroundWindow($hwnd) | Out-Null
    [NV]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 200
    [NV]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    [NV]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
  }
  if (-not (Wait-Log 'nativegallery: saves 2' 10)) { Write-Host "no second save logged" }
  Shot $hwnd "windows-native-after.png"
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Write-Host "=== app log ==="; Get-Content $log
}
$text = Get-Content -Raw $log
if ($text -notmatch 'nativegallery selftest: passed') { $failures += "the self-test failed" }
if ($text -notmatch 'nativegallery: saves 2') { $failures += "the real click on Save didn't reach the app (no 'saves 2')" }
if ($failures.Count -gt 0) { throw ("native views: " + ($failures -join "; ")) }
Write-Host "native views: ok"
