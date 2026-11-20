# The control patterns and events of a running Stipple window (touchdemo),
# through the .NET UI Automation client from this process, as Narrator and
# NVDA use them: the field's Value (read and set) and Text (its document
# range), the "Remember" checkbox's Toggle, the list's and the strip's
# Scroll (to an end and back a page), a row's ScrollItem, and the events a
# tree swap raises: property changes (ToggleState, Name, Value) and
# structure changes, with the toast a tap on "Row 2" raises appearing in the
# tree. UIA's notification event (what announces the toast) has no .NET
# client API, so only the toast's element is checked here; NVDA's script
# listens for its speech.
param(
  [string]$Exe = ".\target\release\examples\touchdemo.exe",
  [string]$Out = "visual_output",
  [string]$Title = "Stipple Touch"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$log = Join-Path $Out "uia-patterns-app.log"
$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
  -RedirectStandardError (Join-Path $Out "uia-patterns-app.err")

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName WindowsBase
# Event handlers that only count (UIA calls them on its own threads, where a
# PowerShell script block has no runspace).
Add-Type -ReferencedAssemblies UIAutomationClient, UIAutomationTypes, WindowsBase @"
using System;
using System.Runtime.InteropServices;
using System.Windows.Automation;
public static class Seen {
  public static int Properties, Structure;
  public static string Last = "";
  public static void OnProperty(object s, AutomationPropertyChangedEventArgs e) {
    System.Threading.Interlocked.Increment(ref Properties);
    Last = e.Property.ProgrammaticName + "=" + e.NewValue;
  }
  public static void OnStructure(object s, StructureChangedEventArgs e) {
    System.Threading.Interlocked.Increment(ref Structure);
  }
  // Subscribe here: Windows PowerShell 5.1 can't turn a static method
  // ([Seen]::OnProperty) into a delegate itself, C# can.
  public static void Watch(AutomationElement win, AutomationProperty[] props) {
    Automation.AddAutomationPropertyChangedEventHandler(win, TreeScope.Subtree,
      new AutomationPropertyChangedEventHandler(OnProperty), props);
    Automation.AddStructureChangedEventHandler(win, TreeScope.Subtree,
      new StructureChangedEventHandler(OnStructure));
  }
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
function Logged($pattern) { Select-String -Path $log -Pattern $pattern -Quiet }
function Await-Log($pattern, [int]$tries = 20) {
  for ($i = 0; $i -lt $tries; $i++) {
    if (Logged $pattern) { return $true }
    Start-Sleep -Milliseconds 250
  }
  $false
}
function FindWindow() {
  for ($i = 0; $i -lt 40; $i++) {
    [IntPtr]$h = [Seen]::FindWindowW([NullString]::Value, $Title)
    $owner = [uint32]0
    if ($h -ne [IntPtr]::Zero) { [void][Seen]::GetWindowThreadProcessId($h, [ref]$owner) }
    if ($h -ne [IntPtr]::Zero -and $owner -eq $proc.Id) { return $A::FromHandle($h) }
    Start-Sleep -Milliseconds 500
  }
  $null
}
# The first element under the window of control type $type whose name
# matches $name, found afresh (the app rebuilds its tree; an element got
# before reads the tree it came from).
function Find($type, $name = '.*') {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, $type)
  $all = $script:win.FindAll($T::Descendants, $cond)
  foreach ($e in $all) { if ($e.Current.Name -match $name) { return $e } }
  $null
}
function Pattern($e, $p) {
  $out = $null
  if ($e -ne $null -and $e.TryGetCurrentPattern($p, [ref]$out)) { return $out }
  $null
}
# The scroll panes: the list (vertical) and the strip (sideways).
function Pane([bool]$horizontal) {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, $C::Pane)
  foreach ($e in $script:win.FindAll($T::Descendants, $cond)) {
    $s = Pattern $e ([System.Windows.Automation.ScrollPattern]::Pattern)
    if ($s -eq $null) { continue }
    if ($horizontal -and $s.Current.HorizontallyScrollable) { return $e }
    if (-not $horizontal -and $s.Current.VerticallyScrollable) { return $e }
  }
  $null
}

try {
  $win = FindWindow
  if ($win -eq $null) { throw "no '$Title' window for process $($proc.Id) through UIA" }
  Start-Sleep -Seconds 2
  $props = @(
    [System.Windows.Automation.TogglePattern]::ToggleStateProperty,
    [System.Windows.Automation.ValuePattern]::ValueProperty,
    $A::NameProperty,
    [System.Windows.Automation.ScrollPattern]::VerticalScrollPercentProperty
  )
  [Seen]::Watch($win, [System.Windows.Automation.AutomationProperty[]]$props)

  # The field: Value and Text.
  $field = Find $C::Edit
  $value = Pattern $field ([System.Windows.Automation.ValuePattern]::Pattern)
  Check ($value -ne $null) "the field has the Value pattern"
  if ($value -ne $null) {
    Write-Host "uia: the field's Value is '$($value.Current.Value)', name '$($field.Current.Name)'"
    $value.SetValue("uia")
    Check (Await-Log '^touchdemo: text "uia"') "SetValue('uia') reached the field: the app logged its text"
    Start-Sleep -Milliseconds 300
    $field = Find $C::Edit
    $value = Pattern $field ([System.Windows.Automation.ValuePattern]::Pattern)
    Check ($value -ne $null -and $value.Current.Value -eq "uia") "the field's Value reads 'uia' again"
    $text = Pattern $field ([System.Windows.Automation.TextPattern]::Pattern)
    Check ($text -ne $null) "the field has the Text pattern"
    if ($text -ne $null) {
      $doc = $text.DocumentRange.GetText(-1)
      Check ($doc -eq "uia") "its DocumentRange reads 'uia' (got '$doc')"
      $sel = $text.GetSelection()
      Check ($sel.Length -eq 1) "GetSelection gives one range (the caret)"
    }
  }

  # The checkbox: Toggle.
  $box = Find $C::CheckBox '^Remember$'
  $toggle = Pattern $box ([System.Windows.Automation.TogglePattern]::Pattern)
  Check ($toggle -ne $null) "a CheckBox 'Remember' with the Toggle pattern"
  if ($toggle -ne $null) {
    Check ($toggle.Current.ToggleState -eq 'Off') "it starts Off"
    $before = [Seen]::Properties
    $toggle.Toggle()
    Check (Await-Log '^touchdemo: remember on') "Toggle reached the app: 'remember on'"
    Start-Sleep -Milliseconds 300
    $toggle = Pattern (Find $C::CheckBox '^Remember$') ([System.Windows.Automation.TogglePattern]::Pattern)
    Check ($toggle -ne $null -and $toggle.Current.ToggleState -eq 'On') "it reads On again"
    Check ([Seen]::Properties -gt $before) "a property-changed event came (last: $([Seen]::Last))"
  }

  # The list of rows: Scroll to the end, a page back, then Row 0 into view.
  $pane = Pane $false
  $scroll = Pattern $pane ([System.Windows.Automation.ScrollPattern]::Pattern)
  Check ($scroll -ne $null) "a vertically scrollable Pane (the list)"
  if ($scroll -ne $null) {
    Write-Host "uia: list scroll $($scroll.Current.VerticalScrollPercent)% (view $($scroll.Current.VerticalViewSize)%)"
    $scroll.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, 100)
    Check (Await-Log '^touchdemo: scrolled to [1-9]') "SetScrollPercent(100) scrolled the list"
    Start-Sleep -Milliseconds 300
    $scroll = Pattern (Pane $false) ([System.Windows.Automation.ScrollPattern]::Pattern)
    Check ($scroll -ne $null -and $scroll.Current.VerticalScrollPercent -gt 99) "the list reads 100% again"
    $row0 = Find $C::Button '^Row 0$'
    $item = Pattern $row0 ([System.Windows.Automation.ScrollItemPattern]::Pattern)
    Check ($item -ne $null) "'Row 0' has the ScrollItem pattern"
    if ($item -ne $null) {
      $item.ScrollIntoView()
      Check (Await-Log '^touchdemo: scrolled to 0$') "ScrollIntoView on 'Row 0' scrolled back to the top"
    }
  }
  $strip = Pattern (Pane $true) ([System.Windows.Automation.ScrollPattern]::Pattern)
  Check ($strip -ne $null) "a sideways scrollable Pane (the strip of chips)"
  if ($strip -ne $null) {
    $strip.Scroll([System.Windows.Automation.ScrollAmount]::LargeIncrement, [System.Windows.Automation.ScrollAmount]::NoAmount)
    Check (Await-Log '^touchdemo: strip scrolled to [1-9]') "Scroll(LargeIncrement) moved the strip"
  }

  # A toast: invoking "Row 2" raises one; it appears in the tree (a
  # structure change on the window).
  Start-Sleep -Milliseconds 300
  $row2 = Find $C::Button '^Row 2$'
  $invoke = Pattern $row2 ([System.Windows.Automation.InvokePattern]::Pattern)
  Check ($invoke -ne $null) "'Row 2' has the Invoke pattern"
  if ($invoke -ne $null) {
    $before = [Seen]::Structure
    $invoke.Invoke()
    Check (Await-Log '^touchdemo: toast "Row 2 tapped"') "the app raised the toast"
    $toast = $null
    for ($i = 0; $i -lt 20 -and $toast -eq $null; $i++) {
      Start-Sleep -Milliseconds 250
      $cond = New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, "Row 2 tapped")
      $toast = $win.FindFirst($T::Descendants, $cond)
    }
    Check ($toast -ne $null) "the toast 'Row 2 tapped' is in the tree"
    Check ([Seen]::Structure -gt $before) "a structure-changed event came ($([Seen]::Structure - $before))"
    Write-Host "note: UIA's notification event (the toast's announcement) has no .NET client API; NVDA's script listens for it"
  }
}
finally {
  [System.Windows.Automation.Automation]::RemoveAllEventHandlers()
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Write-Host "=== touchdemo log ==="
  Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 30
}
if ($failures.Count -gt 0) { throw "UIA pattern checks failed: $($failures -join '; ')" }
Write-Host "UIA pattern checks passed"
