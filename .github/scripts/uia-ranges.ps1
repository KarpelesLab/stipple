# Range values, a table, a caption and text attributes of a running Stipple
# window (touchdemo with TOUCHDEMO_RANGES), through the .NET UI Automation
# client from this process, as Narrator and NVDA read them: the "Volume"
# slider's RangeValue (read, set, a property-changed event), the progress
# bar's (read-only), the "Level" spin button's (set) and its LabeledBy
# caption, the "Items" table's Grid and Table patterns and a cell's GridItem
# and TableItem, a label's text attributes (FontName, FontSize, FontWeight),
# and the field's text-changed and text-selection-changed events.
#
# Then (not failing the script while they're new: each logs "uia-runs: ok:"
# or "uia-runs: FAILED:", and the last line "UIA text runs checks passed")
# the text attribute runs of "Rich bold link bonjour": FindAttribute
# (FontWeight 700) finds "bold", the link's UnderlineStyle and
# ForegroundColor, "bonjour"'s IsItalic and Culture (fr), the mixed value
# of FontWeight over all of it, and a Format unit's expansion.
param(
  [string]$Exe = ".\target\release\examples\touchdemo.exe",
  [string]$Out = "visual_output",
  [string]$Title = "Stipple Touch"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$log = Join-Path $Out "uia-ranges-app.log"
$env:TOUCHDEMO_RANGES = "1"
$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
  -RedirectStandardError (Join-Path $Out "uia-ranges-app.err")

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName WindowsBase
Add-Type -ReferencedAssemblies UIAutomationClient, UIAutomationTypes, WindowsBase @"
using System;
using System.Runtime.InteropServices;
using System.Windows.Automation;
public static class Seen {
  public static int Range, TextChanged, TextSelection;
  public static void OnProperty(object s, AutomationPropertyChangedEventArgs e) {
    System.Threading.Interlocked.Increment(ref Range);
  }
  public static void OnText(object s, AutomationEventArgs e) {
    if (e.EventId == TextPattern.TextChangedEvent) {
      System.Threading.Interlocked.Increment(ref TextChanged);
    } else {
      System.Threading.Interlocked.Increment(ref TextSelection);
    }
  }
  public static void Watch(AutomationElement win) {
    Automation.AddAutomationPropertyChangedEventHandler(win, TreeScope.Subtree,
      new AutomationPropertyChangedEventHandler(OnProperty),
      new AutomationProperty[] { RangeValuePattern.ValueProperty });
    Automation.AddAutomationEventHandler(TextPattern.TextChangedEvent, win,
      TreeScope.Subtree, new AutomationEventHandler(OnText));
    Automation.AddAutomationEventHandler(TextPattern.TextSelectionChangedEvent, win,
      TreeScope.Subtree, new AutomationEventHandler(OnText));
  }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$A = [System.Windows.Automation.AutomationElement]
$T = [System.Windows.Automation.TreeScope]
$C = [System.Windows.Automation.ControlType]
$RV = [System.Windows.Automation.RangeValuePattern]

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
function Find($type, $name = '.*') {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, $type)
  foreach ($e in $script:win.FindAll($T::Descendants, $cond)) {
    if ($e.Current.Name -match $name) { return $e }
  }
  $null
}
$runFailures = @()
function RunCheck($ok, $what) {
  if ($ok) { Write-Host "uia-runs: ok: $what" } else { Write-Host "uia-runs: FAILED: $what"; $script:runFailures += $what }
}
function Pattern($e, $p) {
  $out = $null
  if ($e -ne $null -and $e.TryGetCurrentPattern($p, [ref]$out)) { return $out }
  $null
}

try {
  $win = FindWindow
  if ($win -eq $null) { throw "no '$Title' window for process $($proc.Id) through UIA" }
  Start-Sleep -Seconds 2
  [Seen]::Watch($win)

  # The slider: its range, set to a quarter.
  $slider = Find $C::Slider '^Volume$'
  $range = Pattern $slider $RV::Pattern
  Check ($range -ne $null) "a Slider 'Volume' with the RangeValue pattern"
  if ($range -ne $null) {
    $r = $range.Current
    Write-Host "uia: Volume $($r.Value) in $($r.Minimum)..$($r.Maximum), steps $($r.SmallChange)/$($r.LargeChange), read-only $($r.IsReadOnly)"
    Check ($r.Value -eq 0.5 -and $r.Minimum -eq 0 -and $r.Maximum -eq 1 -and -not $r.IsReadOnly) "it reads 0.5 of 0..1, settable"
    Check ([math]::Abs($r.SmallChange - 0.05) -lt 1e-9) "its small change is 0.05"
    $before = [Seen]::Range
    $range.SetValue(0.25)
    Check (Await-Log '^touchdemo: volume 0.25') "SetValue(0.25) moved the app's slider"
    Start-Sleep -Milliseconds 300
    $range = Pattern (Find $C::Slider '^Volume$') $RV::Pattern
    Check ($range -ne $null -and $range.Current.Value -eq 0.25) "it reads 0.25 again"
    Check ([Seen]::Range -gt $before) "a RangeValue.Value property-changed event came"
  }

  # The progress bar: read-only, following the volume.
  $bar = Pattern (Find $C::ProgressBar) $RV::Pattern
  Check ($bar -ne $null) "a ProgressBar with the RangeValue pattern"
  if ($bar -ne $null) {
    Check ($bar.Current.IsReadOnly -and $bar.Current.Value -eq 0.25) "it is read-only at 0.25 (got $($bar.Current.Value))"
  }

  # The spin button: its caption, its range, set to 7.
  $spin = Find $C::Spinner
  Check ($spin -ne $null) "a Spinner (the 'Level' input)"
  if ($spin -ne $null) {
    $label = $spin.Current.LabeledBy
    Check ($label -ne $null -and $label.Current.Name -eq "Level") "it is LabeledBy 'Level' (got '$($label.Current.Name)')"
    Check ($spin.Current.Name -eq "Level") "...and named by it"
    $sr = Pattern $spin $RV::Pattern
    Check ($sr -ne $null -and $sr.Current.Value -eq 3 -and $sr.Current.Maximum -eq 10) "its RangeValue is 3 of 0..10"
    if ($sr -ne $null) {
      $sr.SetValue(7)
      Check (Await-Log '^touchdemo: level 7') "SetValue(7) set the app's number"
    }
  }

  # The table: Grid, Table, a cell's GridItem and TableItem.
  $table = Find $C::DataGrid '^Items$'
  $grid = Pattern $table ([System.Windows.Automation.GridPattern]::Pattern)
  Check ($grid -ne $null) "a DataGrid 'Items' with the Grid pattern"
  if ($grid -ne $null) {
    Check ($grid.Current.RowCount -eq 21 -and $grid.Current.ColumnCount -eq 2) "it has 21 rows and 2 columns (got $($grid.Current.RowCount)x$($grid.Current.ColumnCount))"
    $cell = $grid.GetItem(1, 0)
    Check ($cell -ne $null -and $cell.Current.Name -eq "Item 0") "GetItem(1, 0) is 'Item 0'"
    $tp = Pattern $table ([System.Windows.Automation.TablePattern]::Pattern)
    Check ($tp -ne $null) "it has the Table pattern"
    if ($tp -ne $null) {
      $heads = @($tp.Current.GetColumnHeaders() | ForEach-Object { $_.Current.Name })
      Check ($heads.Count -eq 2 -and $heads[1] -match '^Count') "its column headers are $($heads -join ', ')"
    }
    if ($cell -ne $null) {
      $gi = Pattern $cell ([System.Windows.Automation.GridItemPattern]::Pattern)
      Check ($gi -ne $null -and $gi.Current.Row -eq 1 -and $gi.Current.Column -eq 0) "the cell's GridItem is at (1, 0)"
      if ($gi -ne $null) {
        Check ($gi.Current.ContainingGrid.Current.Name -eq "Items") "...in the table"
      }
      $ti = Pattern $cell ([System.Windows.Automation.TableItemPattern]::Pattern)
      $ch = @()
      if ($ti -ne $null) { $ch = @($ti.Current.GetColumnHeaderItems() | ForEach-Object { $_.Current.Name }) }
      Check ($ch.Count -eq 1 -and $ch[0] -match '^Item') "its TableItem's column header is '$($ch -join ', ')'"
    }
  }

  # A label's text attributes.
  $caption = Find $C::Text '^Level$'
  $text = Pattern $caption ([System.Windows.Automation.TextPattern]::Pattern)
  Check ($text -ne $null) "the 'Level' label has the Text pattern"
  if ($text -ne $null) {
    $doc = $text.DocumentRange
    $font = $doc.GetAttributeValue([System.Windows.Automation.TextPattern]::FontNameAttribute)
    $size = $doc.GetAttributeValue([System.Windows.Automation.TextPattern]::FontSizeAttribute)
    $weight = $doc.GetAttributeValue([System.Windows.Automation.TextPattern]::FontWeightAttribute)
    Write-Host "uia: 'Level' font '$font', size $size, weight $weight"
    Check (($size -is [double]) -and $size -gt 0) "its FontSize is a size in points ($size)"
    Check ($weight -eq 400) "its FontWeight is 400 ($weight)"
    Check (($font -is [string]) -and $font.Length -gt 0) "its FontName is the app's font ('$font')"
  }

  # Text attribute runs: "Rich bold link bonjour" (plain, bold, plain, a
  # blue underlined link, plain, italic French).
  try {
    $TP = [System.Windows.Automation.TextPattern]
    $rich = Pattern (Find $C::Text '^Rich bold link bonjour$') $TP::Pattern
    RunCheck ($rich -ne $null) "the 'Rich bold link bonjour' text has the Text pattern"
    if ($rich -ne $null) {
      $doc = $rich.DocumentRange
      $bold = $doc.FindAttribute($TP::FontWeightAttribute, 700, $false)
      $boldText = if ($bold -ne $null) { $bold.GetText(-1) } else { $null }
      RunCheck ($boldText -eq "bold") "FindAttribute(FontWeight 700) finds '$boldText'"
      $mixed = $doc.GetAttributeValue($TP::FontWeightAttribute)
      RunCheck ([object]::ReferenceEquals($mixed, $TP::MixedAttributeValue)) "FontWeight over all of it is mixed (got '$mixed')"
      $link = $doc.FindText("link", $false, $false)
      if ($link -ne $null) {
        $under = $link.GetAttributeValue($TP::UnderlineStyleAttribute)
        $color = $link.GetAttributeValue($TP::ForegroundColorAttribute)
        Write-Host "uia-runs: link UnderlineStyle $under, ForegroundColor $('{0:x6}' -f $color)"
        RunCheck ("$under" -eq "Single") "the link is underlined (UnderlineStyle $under)"
        RunCheck ($color -eq 0xd05f1a) "the link's ForegroundColor is 0x1a5fd0 as a COLORREF ($('{0:x6}' -f $color))"
        # A degenerate range in it, expanded to its format run: the link.
        $link.MoveEndpointByRange([System.Windows.Automation.Text.TextPatternRangeEndpoint]::End, $link, [System.Windows.Automation.Text.TextPatternRangeEndpoint]::Start)
        $link.ExpandToEnclosingUnit([System.Windows.Automation.Text.TextUnit]::Format)
        $run = $link.GetText(-1)
        RunCheck ($run -eq "link") "ExpandToEnclosingUnit(Format) at the link is '$run'"
      } else {
        RunCheck $false "FindText('link')"
      }
      $fr = $doc.FindText("bonjour", $false, $false)
      if ($fr -ne $null) {
        $italic = $fr.GetAttributeValue($TP::IsItalicAttribute)
        $culture = $fr.GetAttributeValue($TP::CultureAttribute)
        Write-Host "uia-runs: bonjour IsItalic $italic, Culture $culture"
        RunCheck ($italic -eq $true) "'bonjour' is italic"
        RunCheck ("$culture" -match '^fr') "'bonjour' is French (Culture '$culture')"
      } else {
        RunCheck $false "FindText('bonjour')"
      }
    }
  } catch {
    RunCheck $false "reading the runs threw: $_"
  }
  if ($runFailures.Count -gt 0) {
    Write-Host "UIA text runs checks failed: $($runFailures -join '; ')"
  } else {
    Write-Host "UIA text runs checks passed"
  }

  # The field's text and caret moving: TextChanged and TextSelectionChanged.
  $field = Find $C::Edit
  $value = Pattern $field ([System.Windows.Automation.ValuePattern]::Pattern)
  if ($value -ne $null) {
    $tc = [Seen]::TextChanged
    $ts = [Seen]::TextSelection
    $value.SetValue("ranges")
    Check (Await-Log '^touchdemo: text "ranges"') "SetValue('ranges') reached the field"
    Start-Sleep -Seconds 1
    Check ([Seen]::TextChanged -gt $tc) "a TextChanged event came ($([Seen]::TextChanged - $tc))"
    Check ([Seen]::TextSelection -gt $ts) "a TextSelectionChanged event came ($([Seen]::TextSelection - $ts))"
  }
}
finally {
  [System.Windows.Automation.Automation]::RemoveAllEventHandlers()
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Write-Host "=== touchdemo log ==="
  Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 30
}
if ($failures.Count -gt 0) { throw "UIA range checks failed: $($failures -join '; ')" }
Write-Host "UIA range checks passed"
