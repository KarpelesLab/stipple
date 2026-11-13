# Read a running Stipple window (touchdemo) through UI Automation from this
# process, as Narrator and other screen readers do: the .NET
# System.Windows.Automation client goes through UIA's cross-process channel to
# the window's WM_GETOBJECT provider. Takes the window from its handle (as a
# screen reader does on a focus change) and walks its tree,
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
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
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
# The started process's window, straight from its handle: searching the
# desktop's children would ask every other top-level window for its name, and
# one misbehaving provider elsewhere would break the search.
function FindWindow() {
  for ($i = 0; $i -lt 40; $i++) {
    # The window by its title, made sure to be the started process's
    # (Process.MainWindowHandle came back empty on the runner).
    [IntPtr]$h = [U]::FindWindowW([NullString]::Value, $Title)
    $owner = [uint32]0
    if ($h -ne [IntPtr]::Zero) { [void][U]::GetWindowThreadProcessId($h, [ref]$owner) }
    if ($h -ne [IntPtr]::Zero -and (-not $proc.Id -or $owner -eq $proc.Id)) {
      # The window's element; its name is what the app's provider reports,
      # which needn't be the title, so it's only printed.
      $w = $null
      try { $w = $A::FromHandle($h) } catch { Write-Host "uia: FromHandle($h) failed: $_" }
      if ($w -ne $null) {
        Write-Host ("uia: window 0x{0:x} of process {1}: name=""{2}""" -f [int64]$h, $owner, $w.Current.Name)
        return $w
      }
    } elseif ($i % 10 -eq 9) {
      Write-Host ("uia: waiting for the window: FindWindow 0x{0:x}, owner {1}, started {2}" -f [int64]$h, $owner, $proc.Id)
    }
    Start-Sleep -Milliseconds 500
  }
  $null
}
function Named($elements, $type, $pattern) {
  $elements | Where-Object { $_.Current.ControlType -eq $type -and $_.Current.Name -match $pattern } | Select-Object -First 1
}

try {
  $win = FindWindow
  if ($win -eq $null) { throw "no '$Title' window for process $($proc.Id) through UIA" }
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
    # A hit test finds the top-most window at the point, so the app's must
    # be in front (on the runner, the console can cover it).
    $hwnd = [IntPtr]$win.Current.NativeWindowHandle
    [U]::SetForegroundWindow($hwnd) | Out-Null
    Start-Sleep -Milliseconds 500
    $mid = New-Object System.Windows.Point(($rr.X + $rr.Width / 2), ($rr.Y + $rr.Height / 2))
    if ([U]::GetForegroundWindow() -eq $hwnd) {
      $hit = $A::FromPoint($mid)
      Check ($hit -ne $null -and $hit.Current.Name -eq "Row 0") "the element at 'Row 0''s middle is 'Row 0' (got '$($hit.Current.Name)')"
    } else {
      Write-Host "note: the window isn't in the foreground; hit test not checked"
    }
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
