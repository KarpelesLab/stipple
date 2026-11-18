# The native theme's live controls on Windows (see docs/native-theme.md, "Real
# controls"): run nativegallery's self-test (NATIVEGALLERY_SELFTEST), which
# reads its child-window controls back and acts on them (its text field, an
# EDIT, typed into and set by the app); meanwhile click Save for real
# (SendInput, at the BUTTON child window's middle) and check the app counted
# it, screenshot the window with the self-test's panel over the controls and
# after, walk the window through UI Automation to check each live control is
# one element (the BUTTON's, the EDIT's), not two, and that MSAA (as NVDA
# reads an EDIT) names the field by its hint, pick Small in the select for
# real (a click on its COMBOBOX, then one on Small in the drop-down list
# Windows opens) and check the app took it, then click into the EDIT and
# type into it for real (SendInput), and check the app's state followed.
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
Add-Type -AssemblyName System.Windows.Forms
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
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.IUnknown)] out object acc);
  static Accessibility.IAccessible Acc(IntPtr h) {
    Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
    object o;
    if (h == IntPtr.Zero || AccessibleObjectFromWindow(h, 0xFFFFFFFC, ref iid, out o) != 0) return null;
    return o as Accessibility.IAccessible;
  }
  public static string AccName(IntPtr h) { var a = Acc(h); return a == null ? null : a.get_accName(0); }
  public static string AccRole(IntPtr h) { var a = Acc(h); return a == null ? null : Convert.ToString(a.get_accRole(0)); }
}
"@ -ReferencedAssemblies ([System.Reflection.Assembly]::LoadWithPartialName("Accessibility").Location)
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

  # Screen readers: each live control once, its child window at its node.
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,
    [System.Windows.Automation.Condition]::TrueCondition)
  $saves = 0
  $saveRect = $null
  $edits = 0
  $fieldRect = $null
  foreach ($e in $all) {
    $c = $e.Current
    Write-Host ("uia: {0} {1} '{2}' class={3} hwnd={4}" -f $c.ControlType.ProgrammaticName, $c.LocalizedControlType, $c.Name, $c.ClassName, $c.NativeWindowHandle)
    if ($c.Name -eq "Save") { $saves++; $saveRect = $c.BoundingRectangle }
    # The text field: the window's provider puts the field's node in its
    # EDIT child window's place, once.
    if ($c.ControlType -eq [System.Windows.Automation.ControlType]::Edit) {
      $edits++
      $fieldRect = $c.BoundingRectangle
      Write-Host ("the text field in UIA: '{0}' help '{1}'" -f $c.Name, $c.HelpText)
    }
  }
  if ($saves -ne 1) { $failures += "UIA has $saves elements named Save, not 1" }
  if ($edits -ne 1) { $failures += "UIA has $edits text field elements, not 1" }
  # Its name, as MSAA (and NVDA) reads an EDIT: the field's hint, set with
  # IAccPropServices (an EDIT has no label of its own).
  $edit = [NV]::FindWindowExW($hwnd, [IntPtr]::Zero, "Edit", [NullString]::Value)
  $accName = [NV]::AccName($edit)
  Write-Host "msaa: EDIT $edit name '$accName' role $([NV]::AccRole($edit))"
  if ($accName -ne "Name") { $failures += "MSAA names the EDIT '$accName', not 'Name'" }

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

  # The select is a COMBOBOX child window, read once by UIA: the window's
  # provider puts the child window at the select's node, a combo box named
  # Size (IRawElementProviderHwndOverride).
  $combo = [NV]::FindWindowExW($hwnd, [IntPtr]::Zero, "ComboBox", [NullString]::Value)
  Write-Host "Size COMBOBOX child window: $combo"
  if ($combo -eq [IntPtr]::Zero) { $failures += "no COMBOBOX child window" }
  $selects = @($all | Where-Object {
    $_.Current.Name -eq "Size" -or $_.Current.ClassName -eq "ComboBox" -or
    $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::ComboBox })
  foreach ($e in $selects) {
    Write-Host ("the select in UIA: {0} '{1}' class={2}" -f $e.Current.LocalizedControlType, $e.Current.Name, $e.Current.ClassName)
  }
  if ($selects.Count -ne 1) { $failures += "UIA has $($selects.Count) elements for the select, not 1" }
  # A real pick: click the combo box (Windows opens its own drop-down
  # list), then Small in that list, where UIA says it is.
  if ($combo -ne [IntPtr]::Zero) {
    $r = New-Object RECT
    [NV]::GetWindowRect($combo, [ref]$r) | Out-Null
    $x = [int](($r.Left + $r.Right) / 2); $y = [int](($r.Top + $r.Bottom) / 2)
    Write-Host "clicking the select at $x,$y"
    [NV]::SetForegroundWindow($hwnd) | Out-Null
    [NV]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 200
    [NV]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    [NV]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 800
    Shot $hwnd "windows-native-select-open.png"
    # A list item named Small (the window has a radio button named Small).
    $cond = New-Object System.Windows.Automation.AndCondition(
      (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, "Small")),
      (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::ListItem)))
    $small = [System.Windows.Automation.AutomationElement]::FromHandle($combo).FindFirst(
      [System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($small -eq $null) {
      $small = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
        [System.Windows.Automation.TreeScope]::Descendants, $cond)
    }
    $list = [NV]::FindWindowW("ComboLBox", [NullString]::Value)
    Write-Host "drop-down list window: $list"
    if ($small -ne $null) {
      $b = $small.Current.BoundingRectangle
      $x = [int]($b.X + $b.Width / 2); $y = [int]($b.Y + $b.Height / 2)
    } elseif ($list -ne [IntPtr]::Zero) {
      # Not in UIA's tree: Small is the first row of the drop-down list
      # (Windows' ComboLBox popup), one item high.
      $lr = New-Object RECT
      [NV]::GetWindowRect($list, [ref]$lr) | Out-Null
      $ih = [int][NV]::SendMessageW($list, 0x01A1, [IntPtr]::Zero, [IntPtr]::Zero) # LB_GETITEMHEIGHT
      if ($ih -le 0) { $ih = 16 }
      $x = [int](($lr.Left + $lr.Right) / 2); $y = [int]($lr.Top + 1 + $ih / 2)
    } else {
      $failures += "the select's drop-down list didn't open"
    }
    if ($small -ne $null -or $list -ne [IntPtr]::Zero) {
      Write-Host "clicking Small at $x,$y"
      [NV]::SetCursorPos($x, $y) | Out-Null
      Start-Sleep -Milliseconds 200
      [NV]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 100
      [NV]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    }
    if (-not (Wait-Log 'nativegallery: size Some\(0\)' 10)) { Write-Host "no pick of Small logged" }
  }

  # Real typing into the EDIT: a click into it, then keys (SendInput).
  if ($fieldRect -ne $null) {
    $x = [int]($fieldRect.X + $fieldRect.Width - 8); $y = [int]($fieldRect.Y + $fieldRect.Height / 2)
    Write-Host "clicking the text field at $x,$y and typing"
    [NV]::SetForegroundWindow($hwnd) | Out-Null
    [NV]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 200
    [NV]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    [NV]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 300
    [System.Windows.Forms.SendKeys]::SendWait("{END}xy")
    if (-not (Wait-Log 'nativegallery: name ".*xy"' 10)) { $failures += "typing into the EDIT didn't reach the app (no 'name ...xy')" }
  }
  Shot $hwnd "windows-native-after.png"
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Write-Host "=== app log ==="; Get-Content $log
}
$text = Get-Content -Raw $log
if ($text -notmatch 'nativegallery selftest: passed') { $failures += "the self-test failed" }
if ($text -notmatch 'nativegallery: saves 2') { $failures += "the real click on Save didn't reach the app (no 'saves 2')" }
if ($text -notmatch 'nativegallery: size Some\(0\)') { $failures += "the real pick of Small didn't reach the app (no 'size Some(0)')" }
if ($failures.Count -gt 0) { throw ("native views: " + ($failures -join "; ")) }
Write-Host "native views: ok"
