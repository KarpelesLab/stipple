# Read a running Stipple window (touchdemo) through UI Automation from this
# process, as Narrator and other screen readers do: the .NET
# System.Windows.Automation client goes through UIA's cross-process channel to
# the window's WM_GETOBJECT provider. Walks the tree from the desktop root,
# checks the controls a screen reader would land on (with their bounds on
# screen and a hit test at one), invokes a button through its Invoke pattern
# and sees the app (and the tree) follow, then moves keyboard focus with Tab
# and asks UIA which element has it.
param(
  [string]$Exe = ".\target\release\touchdemo.exe",
  [string]$Out = "visual_output",
  [string]$Title = "Stipple Touch"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$log = Join-Path $Out "uia-walk-app.log"
$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
  -RedirectStandardError (Join-Path $Out "uia-walk-app.err")

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class U {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
"@
$A = [System.Windows.Automation.AutomationElement]
$T = [System.Windows.Automation.TreeScope]
$C = [System.Windows.Automation.ControlType]

$failures = @()
function Check($ok, $what) {
  if ($ok) { Write-Host "ok: $what" } else { Write-Host "FAILED: $what"; $script:failures += $what }
}
function Describe($e) {
  $c = $e.Current
  $r = $c.BoundingRectangle
  $patterns = ($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Identifiers.Pattern', '' }) -join ','
  "{0} name=""{1}"" rect={2},{3},{4}x{5} focusable={6} focused={7} patterns=[{8}]" -f `
    ($c.ControlType.ProgrammaticName -replace 'ControlType.', ''), $c.Name, `
    [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height, `
    $c.IsKeyboardFocusable, $c.HasKeyboardFocus, $patterns
}
# Every element under $root in the control view, depth first, as a screen
# reader's linear navigation reads them.
function Walk($root) {
  $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
  $found = New-Object System.Collections.ArrayList
  $stack = New-Object System.Collections.Stack
  $stack.Push(@($root, 0))
  while ($stack.Count -gt 0) {
    $item = $stack.Pop()
    $e = $item[0]; $depth = $item[1]
    [void]$found.Add($e)
    Write-Host ("uia: " + ("  " * $depth) + (Describe $e))
    $kids = @()
    $c = $walker.GetFirstChild($e)
    while ($c -ne $null) { $kids += $c; $c = $walker.GetNextSibling($c) }
    for ($i = $kids.Count - 1; $i -ge 0; $i--) { $stack.Push(@($kids[$i], ($depth + 1))) }
  }
  $found
}
function FindWindow() {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, $Title)
  for ($i = 0; $i -lt 40; $i++) {
    $w = $A::RootElement.FindFirst($T::Children, $cond)
    if ($w -ne $null) { return $w }
    Start-Sleep -Milliseconds 500
  }
  $null
}
function Named($elements, $type, $pattern) {
  $elements | Where-Object { $_.Current.ControlType -eq $type -and $_.Current.Name -match $pattern } | Select-Object -First 1
}

try {
  $win = FindWindow
  if ($win -eq $null) { throw "no '$Title' window under the UIA desktop root" }
  Start-Sleep -Seconds 2
  $elements = Walk $win
  Write-Host "uia: $($elements.Count) elements under '$Title'"
  $row0 = Named $elements $C::Button '^Row 0$'
  $field = Named $elements $C::Edit '.*'
  $count = Named $elements $C::Text '^Rows tapped: \d+$'
  Check ($row0 -ne $null) "a Button named 'Row 0'"
  Check ($field -ne $null) "an Edit control (the text field)"
  Check ($count -ne $null) "a Text 'Rows tapped: N'"
  if ($row0 -ne $null) {
    $wr = $win.Current.BoundingRectangle
    $rr = $row0.Current.BoundingRectangle
    Check ($rr.Width -gt 0 -and $rr.Height -gt 0) "'Row 0' has a size on screen"
    Check ($wr.Contains($rr)) "'Row 0' ($rr) lies inside its window ($wr) on screen"
    # A hit test at its middle (as Narrator's mouse and touch reading do)
    # lands on it.
    $mid = New-Object System.Windows.Point(($rr.X + $rr.Width / 2), ($rr.Y + $rr.Height / 2))
    $hit = $A::FromPoint($mid)
    Check ($hit -ne $null -and $hit.Current.Name -eq "Row 0") "the element at 'Row 0''s middle is 'Row 0' (got '$($hit.Current.Name)')"
    $before = [int]($count.Current.Name -replace '^Rows tapped: ', '')
    $invoke = $row0.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $invoke.Invoke()
    $want = "Rows tapped: $($before + 1)"
    $after = $null
    for ($i = 0; $i -lt 20 -and $after -eq $null; $i++) {
      Start-Sleep -Milliseconds 250
      $cond = New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, $want)
      $after = $win.FindFirst($T::Descendants, $cond)
    }
    Check ($after -ne $null) "Invoke on 'Row 0' reached the app: the tree now reads '$want'"
    Check ((Select-String -Path $log -Pattern '^touchdemo: row 0 tapped' -Quiet)) "the app logged the tap"
  }

  # Keyboard focus, as a screen reader follows it: Tab into the window, then
  # ask UIA for the focused element.
  $hwnd = [IntPtr]$win.Current.NativeWindowHandle
  [U]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 500
  [System.Windows.Forms.SendKeys]::SendWait("{TAB}")
  Start-Sleep -Milliseconds 700
  $focused = $A::FocusedElement
  Write-Host "uia: focused $(Describe $focused)"
  if ([U]::GetForegroundWindow() -eq $hwnd) {
    Check ($focused.Current.HasKeyboardFocus -and $focused.Current.ControlType -ne $C::Window) "after Tab, UIA's focused element is a control in the window"
  } else {
    Write-Host "note: the window isn't in the foreground; focus not checked"
  }
}
finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Write-Host "=== touchdemo log ==="
  Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 30
  Get-Content (Join-Path $Out "uia-walk-app.err") -ErrorAction SilentlyContinue | Select-Object -Last 20
}
if ($failures.Count -gt 0) { throw "UIA checks failed: $($failures -join '; ')" }
