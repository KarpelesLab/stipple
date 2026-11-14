# Drive a running Stipple window (touchdemo) with a real screen reader: NVDA,
# downloaded from nvaccess.org (a pinned release, checked against its
# SHA-256), made into a portable copy and started with a fresh configuration:
# no dialogs, a synth that makes no sound (.github/a11y/windows/stippleSpy.py,
# loaded from the configuration's scratchpad) and its log at debug level.
# NVDA logs everything it says as "Speaking [...]" (input/output level), so
# the checks read its log:
#
#   1. bringing the window to the front: NVDA names it ("Stipple Touch");
#   2. Tab: the app moves focus to its text field and raises UIA's
#      focus-changed event, and NVDA says "edit";
#   3. NVDA+B (read the foreground window): NVDA reads the window's controls
#      through UIA, "Row 0" with its role "button" and "Rows tapped: 0";
#   4. NVDA+Shift+Right (next object, laptop layout) from the field until
#      NVDA's navigator says "Row 0 button";
#   5. NVDA+Enter (activate the navigator object): NVDA invokes the button
#      (UIA Invoke), the app logs the tap, and NVDA+B then reads
#      "Rows tapped: 1";
#   6. the keyboard alone: Tab from the field to "Row 0" (the app moves its
#      focus to the button and raises UIA's focus-changed event; NVDA says
#      "Row 0 button"), Space presses it and Enter presses it again (the app
#      logs both taps), and Shift+Tab goes back to the field ("edit");
#   7. the mouse moved over "Row 1": NVDA's mouse tracking hit-tests through
#      UIA and says "Row 1" (reported, not required).
#
# NVDA's key here is Insert. Keys are typed with keybd_event, so they reach
# NVDA's keyboard hook as typed keys (NVDA handles injected keys by default).
param(
  [string]$Exe = ".\target\release\touchdemo.exe",
  [string]$Out = "visual_output",
  [string]$Title = "Stipple Touch",
  [string]$Synth = ".github\a11y\windows\stippleSpy.py",
  [string]$Version = "2025.3.2",
  # sha256sum of nvda_2025.3.2.exe from download.nvaccess.org.
  [string]$Sha256 = "29f4388bc45dffbac79de11622813d81c4dd21d7e403e7ae869b0537732fcbdb"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$work = Join-Path $(if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }) "nvda-ci"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$launcher = Join-Path $work "nvda_$Version.exe"
$portable = Join-Path $work "portable"
$config = Join-Path $work "config"
$nvdaLog = Join-Path $Out "nvda.log"
$appLog = Join-Path $Out "nvda-touchdemo.log"

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class NV {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
}
"@

$failures = @()
$notes = @()
function Check($ok, $what) {
  if ($ok) { Write-Host "ok: $what" } else { Write-Host "FAILED: $what"; $script:failures += $what }
}
function Note($ok, $what) {
  if ($ok) { Write-Host "ok: $what" } else { Write-Host "note (not required): $what didn't happen"; $script:notes += $what }
}

# ---- NVDA ------------------------------------------------------------------

function Get-Nvda {
  if (-not (Test-Path $launcher)) {
    $url = "https://download.nvaccess.org/releases/$Version/nvda_$Version.exe"
    for ($i = 1; $i -le 3; $i++) {
      try {
        Write-Host "nvda: downloading $url (try $i)"
        $ProgressPreference = "SilentlyContinue"
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $url -OutFile $launcher -UseBasicParsing
        break
      } catch {
        Write-Host "nvda: download failed: $_"
        Remove-Item $launcher -ErrorAction SilentlyContinue
        if ($i -eq 3) { throw "couldn't download NVDA $Version" }
        Start-Sleep -Seconds 10
      }
    }
  }
  $hash = (Get-FileHash -Algorithm SHA256 $launcher).Hash.ToLower()
  Write-Host "nvda: launcher $((Get-Item $launcher).Length) bytes, sha256 $hash"
  if ($hash -ne $Sha256) { throw "NVDA launcher's SHA-256 is $hash, expected $Sha256" }
}

function Stop-Nvda {
  $running = Get-Process -Name nvda, nvda_noUIAccess, nvda_uiAccess -ErrorAction SilentlyContinue
  if ($running) {
    Write-Host "nvda: stopping $(($running | ForEach-Object { "$($_.Name)#$($_.Id)" }) -join ', ')"
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
  }
}

# The portable copy: the launcher runs NVDA from a temporary folder, which
# copies itself to --portable-path and exits (no dialog with -silent).
function New-Portable {
  if (Test-Path (Join-Path $portable "nvda.exe")) { return (Join-Path $portable "nvda.exe") }
  $createLog = Join-Path $Out "nvda-create-portable.log"
  $cpArgs = @("--create-portable-silent", "--portable-path=`"$portable`"", "--minimal",
    "--log-file=`"$createLog`"", "--log-level=20", "--disable-addons")
  Write-Host "nvda: $launcher $($cpArgs -join ' ')"
  $p = Start-Process -FilePath $launcher -ArgumentList $cpArgs -PassThru
  $deadline = (Get-Date).AddSeconds(240)
  while (-not $p.HasExited -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }
  if (-not $p.HasExited) {
    Write-Host "nvda: the launcher is still running after 240 s; stopping it"
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  } else {
    Write-Host "nvda: the launcher exited with $($p.ExitCode)"
  }
  Stop-Nvda
  if (Test-Path $createLog) {
    Write-Host "=== NVDA portable creation log (tail) ==="
    Get-Content $createLog -Tail 40 | ForEach-Object { Write-Host $_ }
  }
  $exe = Join-Path $portable "nvda.exe"
  if (Test-Path $exe) { return $exe }
  # The launcher is an NSIS archive; 7-Zip (on the runner image) unpacks the
  # NVDA it carries, which runs as well from there.
  Write-Host "nvda: no portable copy at $portable; unpacking the launcher with 7-Zip instead"
  $seven = @("C:\Program Files\7-Zip\7z.exe", "7z.exe") | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue } | Select-Object -First 1
  if (-not $seven) { throw "no portable NVDA and no 7-Zip to unpack the launcher" }
  $unpacked = Join-Path $work "unpacked"
  & $seven x -y "-o$unpacked" $launcher | Select-Object -Last 3 | ForEach-Object { Write-Host $_ }
  $found = Get-ChildItem -Path $unpacked -Recurse -Include nvda_noUIAccess.exe, nvda.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $found) { throw "the unpacked launcher has no nvda.exe" }
  return $found.FullName
}

# A configuration with no speech output (the stippleSpy synth, from the
# scratchpad; NVDA's "silence" synth never reports its speech done, which
# stops say all), no welcome / usage / update dialogs, the laptop keyboard
# layout (NVDA+Shift+arrows move the object navigator) and Insert as the NVDA
# key. schemaVersion is the one this NVDA release writes. (No
# --disable-addons: it turns the scratchpad off too; the configuration is
# new, with no add-ons.)
function Write-Config {
  New-Item -ItemType Directory -Force -Path $config | Out-Null
  $drivers = Join-Path $config "scratchpad\synthDrivers"
  New-Item -ItemType Directory -Force -Path $drivers | Out-Null
  Copy-Item -Path $Synth -Destination (Join-Path $drivers "stippleSpy.py") -Force
  $ini = @"
schemaVersion = 18
[general]
	language = en
	showWelcomeDialogAtStartup = False
	playStartAndExitSounds = False
	askToExit = False
	loggingLevel = DEBUG
[speech]
	synth = stippleSpy
[development]
	enableScratchpadDir = True
[keyboard]
	keyboardLayout = laptop
	NVDAModifierKeys = 6
	handleInjectedKeys = True
	speakTypedCharacters = 0
[mouse]
	enableMouseTracking = True
[update]
	autoCheck = False
	startupNotification = False
	allowUsageStats = False
	askedAllowUsageStats = True
[addonStore]
	automaticUpdates = disabled
[debugLog]
	UIA = True
	events = True
"@
  Set-Content -Path (Join-Path $config "nvda.ini") -Value $ini -Encoding ASCII
}

# NVDA's log, read while NVDA writes it.
function Read-NvdaLog {
  if (-not (Test-Path $nvdaLog)) { return "" }
  try {
    $fs = [System.IO.File]::Open($nvdaLog, 'Open', 'Read', 'ReadWrite')
    try {
      $sr = New-Object System.IO.StreamReader($fs, [System.Text.Encoding]::UTF8)
      return $sr.ReadToEnd()
    } finally { $fs.Close() }
  } catch { return "" }
}
# What NVDA said since $from (an offset into the log): its "Speaking [...]" lines.
function Spoken($from) {
  $text = Read-NvdaLog
  if ($text.Length -le $from) { return @() }
  @($text.Substring($from) -split "`r?`n" | Where-Object { $_ -match '^Speaking \[' })
}
function Mark { (Read-NvdaLog).Length }
# Wait until NVDA says something matching every pattern in $all on one line,
# since $from; print what it said meanwhile.
function Await-Speech($from, [string[]]$all, $what, [int]$seconds = 8) {
  $deadline = (Get-Date).AddSeconds($seconds)
  $hit = $null
  do {
    Start-Sleep -Milliseconds 400
    $lines = Spoken $from
    $hit = $lines | Where-Object { $l = $_; @($all | Where-Object { $l -notmatch $_ }).Count -eq 0 } | Select-Object -First 1
  } while (-not $hit -and (Get-Date) -lt $deadline)
  foreach ($l in $lines) { Write-Host "nvda said: $l" }
  if (-not $lines) { Write-Host "nvda said nothing ($what)" }
  return $hit
}

# ---- keys --------------------------------------------------------------------

$VK = @{ Insert = 0x2D; Shift = 0xA0; Right = 0x27; Down = 0x28; Enter = 0x0D; Tab = 0x09; B = 0x42; Backspace = 0x08; Control = 0xA2; Space = 0x20 }
$SCAN = @{ Insert = 0x52; Shift = 0x2A; Right = 0x4D; Down = 0x50; Enter = 0x1C; Tab = 0x0F; B = 0x30; Backspace = 0x0E; Control = 0x1D; Space = 0x39 }
$EXTENDED = @{ Insert = $true; Right = $true; Down = $true }
function Key-Event($name, [bool]$up) {
  $flags = 0
  if ($EXTENDED[$name]) { $flags = $flags -bor 1 }
  if ($up) { $flags = $flags -bor 2 }
  [NV]::keybd_event([byte]$VK[$name], [byte]$SCAN[$name], [uint32]$flags, [UIntPtr]::Zero)
}
# Press the keys in $names together (modifiers first), then release them.
function Chord([string[]]$names) {
  Write-Host "keys: $($names -join '+')"
  foreach ($n in $names) { Key-Event $n $false; Start-Sleep -Milliseconds 40 }
  [array]::Reverse($names)
  foreach ($n in $names) { Key-Event $n $true; Start-Sleep -Milliseconds 40 }
  Start-Sleep -Milliseconds 300
}

function Find-AppWindow($proc) {
  for ($i = 0; $i -lt 60; $i++) {
    [IntPtr]$h = [NV]::FindWindowW([NullString]::Value, $Title)
    $owner = [uint32]0
    if ($h -ne [IntPtr]::Zero) { [void][NV]::GetWindowThreadProcessId($h, [ref]$owner) }
    if ($h -ne [IntPtr]::Zero -and $owner -eq $proc.Id) { return $h }
    Start-Sleep -Milliseconds 500
  }
  [IntPtr]::Zero
}
function Front($hwnd) {
  for ($i = 0; $i -lt 5; $i++) {
    [void][NV]::ShowWindow($hwnd, 9)  # SW_RESTORE
    [void][NV]::SetForegroundWindow($hwnd)
    Start-Sleep -Milliseconds 600
    if ([NV]::GetForegroundWindow() -eq $hwnd) { return $true }
  }
  Write-Host ("note: the foreground window is 0x{0:x}, not the app's 0x{1:x}" -f [int64][NV]::GetForegroundWindow(), [int64]$hwnd)
  $false
}

# ---- the run -----------------------------------------------------------------

$app = $null
$nvda = $null
$nvdaExe = $null
try {
  Get-Nvda
  $nvdaExe = New-Portable
  Write-Host "nvda: $nvdaExe"
  Write-Config
  Stop-Nvda

  $app = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $appLog `
    -RedirectStandardError (Join-Path $Out "nvda-touchdemo.err")
  $hwnd = Find-AppWindow $app
  if ($hwnd -eq [IntPtr]::Zero) { throw "no '$Title' window for process $($app.Id)" }
  Write-Host ("app: window 0x{0:x} of process {1}" -f [int64]$hwnd, $app.Id)
  Start-Sleep -Seconds 2

  Remove-Item $nvdaLog -ErrorAction SilentlyContinue
  $nvArgs = @("-m", "--config-path=`"$config`"", "--log-file=`"$nvdaLog`"", "--log-level=10")
  Write-Host "nvda: starting $nvdaExe $($nvArgs -join ' ')"
  $nvda = Start-Process -FilePath $nvdaExe -ArgumentList $nvArgs -PassThru
  $deadline = (Get-Date).AddSeconds(90)
  while (((Read-NvdaLog) -notmatch 'NVDA initialized') -and (Get-Date) -lt $deadline -and -not $nvda.HasExited) {
    Start-Sleep -Seconds 1
  }
  $log = Read-NvdaLog
  if ($log -notmatch 'NVDA initialized') {
    Write-Host "=== NVDA log ==="; Write-Host $log
    throw "NVDA didn't come up (exited: $($nvda.HasExited))"
  }
  Write-Host "nvda: initialized (process $($nvda.Id))"
  foreach ($l in ($log -split "`r?`n" | Where-Object { $_ -match 'synth|stippleSpy|Scratchpad|Config dir|Using .*version|Windows version' } | Select-Object -First 16)) {
    Write-Host "nvda log: $l"
  }
  Check ($log -match 'stippleSpy: ready') "NVDA loaded the stippleSpy synth from the scratchpad"
  Start-Sleep -Seconds 3

  # 1. The window comes to the front: NVDA names it. (Minimized first: a
  # window already in front raises no foreground event.)
  [void][NV]::ShowWindow($hwnd, 6)  # SW_MINIMIZE
  Start-Sleep -Seconds 1
  $m = Mark
  $front = Front $hwnd
  Check $front "the app's window is in the foreground"
  $hit = Await-Speech $m @('Stipple Touch') "foreground" 10
  # Seen in CI: NVDA names the window only with the first focus inside it
  # (on the Tab below), not on the foreground change itself. Reported, not
  # required.
  if ($hit -ne $null) { Write-Host "ok: NVDA named the window 'Stipple Touch' as it came to the front" }
  else { Write-Host "note (not required): NVDA didn't name the window on the foreground change alone" }

  # 2. Tab focuses the text field; the UIA focus event makes NVDA say so.
  [void](Front $hwnd)
  $m = Mark
  Chord @('Tab')
  $hit = Await-Speech $m @("'edit'") "Tab"
  Check ($hit -ne $null) "after Tab, NVDA announced the focused field ('edit')"
  # NVDA+Tab: report the focus again, as NVDA asks for it.
  $m = Mark
  Chord @('Insert', 'Tab')
  $hit = Await-Speech $m @("'edit'") "NVDA+Tab"
  Check ($hit -ne $null) "NVDA+Tab reports the focused field ('edit')"

  # 3. NVDA+B reads the whole window.
  $m = Mark
  Chord @('Insert', 'B')
  $hit = Await-Speech $m @('Row 0') "NVDA+B" 12
  Check ($hit -ne $null) "NVDA+B read 'Row 0'"
  $all = Spoken $m
  $rowLine = @($all | Where-Object { $_ -match "'Row 0'" -and $_ -match "'button'" }).Count -gt 0
  Check $rowLine "NVDA+B read 'Row 0' as a button (its name and role in one utterance)"
  Check (($all -join ' ') -match 'Rows tapped: 0') "NVDA+B read 'Rows tapped: 0'"

  # 4. The object navigator, from the field, to "Row 0". NVDA+B's say-all
  # leaves the navigator on the last object it read: stop it (Control) and
  # bring the navigator back to the focus (NVDA+Backspace, laptop layout).
  Chord @('Control')
  $m = Mark
  Chord @('Insert', 'Backspace')
  $hit = Await-Speech $m @("'edit'") "navigator to focus" 4
  if ($hit -eq $null) { Write-Host "note: NVDA+Backspace didn't report the field" }
  $onRow = $false
  for ($i = 0; $i -lt 12 -and -not $onRow; $i++) {
    $m = Mark
    Chord @('Insert', 'Shift', 'Right')
    $hit = Await-Speech $m @('.') "next object" 3
    if ($hit -match "'Row 0'") { $onRow = $true }
    elseif ($hit -match 'No next|no next') {
      # The end of a level: into the next one down.
      $m = Mark
      Chord @('Insert', 'Shift', 'Down')
      $hit = Await-Speech $m @('.') "first child" 3
      if ($hit -match "'Row 0'") { $onRow = $true }
    }
  }
  Check $onRow "NVDA's object navigation reached 'Row 0'"
  if ($onRow) {
    Check ($hit -match "'button'") "NVDA said 'Row 0' is a button"

    # 5. NVDA+Enter presses it, through UIA's Invoke.
    $before = @(Select-String -Path $appLog -Pattern '^touchdemo: row 0 tapped').Count
    $m = Mark
    Chord @('Insert', 'Enter')
    $tapped = $false
    for ($i = 0; $i -lt 20 -and -not $tapped; $i++) {
      Start-Sleep -Milliseconds 250
      $tapped = @(Select-String -Path $appLog -Pattern '^touchdemo: row 0 tapped').Count -gt $before
    }
    foreach ($l in (Spoken $m)) { Write-Host "nvda said: $l" }
    Check $tapped "NVDA+Enter on 'Row 0' pressed it: the app logged the tap"
    if ($tapped) {
      [void](Front $hwnd)
      $m = Mark
      Chord @('Insert', 'B')
      $hit = Await-Speech $m @('Rows tapped: 1') "NVDA+B after the press" 12
      Check ($hit -ne $null) "NVDA+B then read 'Rows tapped: 1'"
    }
  }

  # 6. The keyboard alone: Tab reaches "Row 0" (from wherever focus is: the
  # press above was a click, which leaves a button unfocused), Space and
  # Enter press it, Shift+Tab goes back.
  [void](Front $hwnd)
  $onButton = $false
  for ($i = 0; $i -lt 4 -and -not $onButton; $i++) {
    $m = Mark
    Chord @('Tab')
    $hit = Await-Speech $m @("'Row 0'") "Tab to Row 0" 3
    $onButton = $hit -ne $null
  }
  Check $onButton "Tab reached 'Row 0' and NVDA announced it"
  if ($onButton) {
    Check ($hit -match "'button'") "NVDA said the focused 'Row 0' is a button"
    foreach ($key in @('Space', 'Enter')) {
      $before = @(Select-String -Path $appLog -Pattern '^touchdemo: row 0 tapped').Count
      Chord @($key)
      $tapped = $false
      for ($i = 0; $i -lt 20 -and -not $tapped; $i++) {
        Start-Sleep -Milliseconds 250
        $tapped = @(Select-String -Path $appLog -Pattern '^touchdemo: row 0 tapped').Count -gt $before
      }
      Check $tapped "$key on the focused 'Row 0' pressed it: the app logged the tap"
    }
    $m = Mark
    Chord @('Shift', 'Tab')
    $hit = Await-Speech $m @("'edit'") "Shift+Tab" 4
    Check ($hit -ne $null) "Shift+Tab went back to the field ('edit')"
  }

  # 7. The mouse over "Row 1": NVDA's mouse tracking says what is there.
  Add-Type -AssemblyName UIAutomationClient
  try {
    $win = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
    $cond = New-Object System.Windows.Automation.PropertyCondition(
      [System.Windows.Automation.AutomationElement]::NameProperty, "Row 1")
    $row1 = $win.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($row1 -ne $null) {
      $r = $row1.Current.BoundingRectangle
      $m = Mark
      [void][NV]::SetCursorPos([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2) - 1)
      Start-Sleep -Milliseconds 200
      [void][NV]::SetCursorPos([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
      $hit = Await-Speech $m @('Row 1') "mouse over Row 1" 5
      Note ($hit -ne $null) "NVDA's mouse tracking said 'Row 1' under the pointer"
    } else {
      Write-Host "note: no 'Row 1' through UIA for the mouse check"
    }
  } catch { Write-Host "note: mouse check failed: $_" }
}
finally {
  if ($nvda -ne $null -and -not $nvda.HasExited) {
    # nvda -q asks the running copy to quit.
    try {
      $q = Start-Process -FilePath $nvdaExe -ArgumentList "-q" -PassThru
      [void]$q.WaitForExit(20000)
    } catch { Write-Host "nvda: -q failed: $_" }
    Start-Sleep -Seconds 2
  }
  Stop-Nvda
  if ($app -ne $null) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 1
  Write-Host "=== everything NVDA said ==="
  Spoken 0 | ForEach-Object { Write-Host $_ }
  Write-Host "=== NVDA log: warnings and errors ==="
  (Read-NvdaLog) -split "`r?`n" | Where-Object { $_ -match '^(ERROR|WARNING|CRITICAL) ' } | Select-Object -First 40 | ForEach-Object { Write-Host $_ }
  Write-Host "=== touchdemo log (tail) ==="
  Get-Content $appLog -ErrorAction SilentlyContinue | Select-Object -Last 20
  Get-Content (Join-Path $Out "nvda-touchdemo.err") -ErrorAction SilentlyContinue | Select-Object -Last 20
}
if ($notes.Count -gt 0) { Write-Host "notes: $($notes -join '; ')" }
if ($failures.Count -gt 0) { throw "NVDA checks failed: $($failures -join '; ')" }
Write-Host "NVDA checks passed"
