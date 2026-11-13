# Type Japanese into examples/textinput through the real Microsoft Japanese
# IME, by way of TSF.
#
#   .github\scripts\ime-windows.ps1 <textinput.exe> [out dir]
#
# 1. The IME: the Japanese language's typing feature carries it. If its TSF
#    profile isn't registered, try to add it (Add-WindowsCapability, then
#    Install-Language), with Windows Update started if the image stopped it.
# 2. The user's language list gets ja-JP with the IME as its input method.
# 3. The app starts and is brought to the foreground (without leaving an Alt
#    tap behind: a lone Alt release puts the window in menu mode, which eats
#    every key after it), Tab focuses the field (TSF ties the window to its
#    document: the app's trace says "focus"), and WM_INPUTLANGCHANGEREQUEST
#    switches the window's thread to Japanese, which activates the IME's
#    profile.
# 4. SendInput: VK_IME_ON (or VK_KANJI) until "a" becomes a preedit, Escape
#    cancels it (again if the first Escape only closed the IME's prediction
#    list), then n i h o n (a hiragana preedit), Space (the conversion,
#    nihon in kanji), Return (the commit). The app prints its field
#    (STIPPLE_TEXTINPUT_ECHO) and its TSF trace (STIPPLE_DEBUG_TSF).
#
# Every stage waits for the app to show its effect and says so; a run that
# fails names the stage ("FAILED at <stage>") and the window's input state
# (foreground, focus, menu mode, Alt held). Setting up (foreground, the
# field's focus, the IME's on state, an empty field) is retried; what is
# under test (the preedit, the conversion, the commit) gets one retry of the
# whole word from an empty field. Exit code 0 only when the commit arrived.
param([string]$App, [string]$Out = "visual_output")

$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$tip = '{03B5835F-F03C-411B-9CE2-AA23E1171E36}'      # Microsoft IME (Japanese)
$profileGuid = '{A76C93D9-5523-4E90-AAFA-4DB112F9AC76}'
$profileKey = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$tip\LanguageProfile\0x00000411\$profileGuid"
$title = 'Stipple Text Input'

function Stage($msg) { Write-Host "ime-windows: $msg" }
function Has-Ime { Test-Path $profileKey }

# Runs a script block with a deadline (installs can stall on the runner).
function With-Deadline([scriptblock]$Block, [int]$Seconds, [string]$What) {
  $job = Start-Job -ScriptBlock $Block
  if (Wait-Job $job -Timeout $Seconds) {
    Receive-Job $job 2>&1 | ForEach-Object { Write-Host "  $_" }
  } else {
    Stage "$What did not finish in $Seconds s"
    Stop-Job $job
  }
  Remove-Job $job -Force
}

$os = Get-CimInstance Win32_OperatingSystem
Stage "$($os.Caption) $($os.Version), UI language $((Get-UICulture).Name)"

# --- 1. the IME ---------------------------------------------------------------
if (Has-Ime) {
  Stage "the Japanese IME's TSF profile is registered"
} else {
  Stage "the Japanese IME's TSF profile isn't registered; adding the ja-JP typing feature"
  Get-WindowsCapability -Online -Name 'Language.Basic~~~ja-JP*' -ErrorAction SilentlyContinue |
    ForEach-Object { Stage "capability $($_.Name): $($_.State)" }
  $wu = Get-Service wuauserv -ErrorAction SilentlyContinue
  if ($wu) {
    Stage "Windows Update service: $($wu.Status), start type $($wu.StartType)"
    if ($wu.StartType -eq 'Disabled') { Set-Service wuauserv -StartupType Manual }
    if ($wu.Status -ne 'Running') { Start-Service wuauserv -ErrorAction SilentlyContinue }
  }
  With-Deadline { Add-WindowsCapability -Online -Name 'Language.Basic~~~ja-JP~0.0.1.0' | Format-List } 900 'Add-WindowsCapability'
  if (-not (Has-Ime)) {
    if (Get-Command Install-Language -ErrorAction SilentlyContinue) {
      With-Deadline { Install-Language -Language ja-JP | Format-List } 900 'Install-Language'
    } else {
      Stage "Install-Language isn't available on this image"
    }
  }
  if (-not (Has-Ime)) {
    Get-WindowsCapability -Online -Name 'Language.Basic~~~ja-JP*' -ErrorAction SilentlyContinue |
      ForEach-Object { Stage "capability $($_.Name): $($_.State)" }
    Stage "FAILED at install: no Japanese IME could be installed on this runner"
    exit 1
  }
  Stage "the Japanese IME is installed"
}

# --- 2. the user's languages ------------------------------------------------
$list = Get-WinUserLanguageList
if (-not ($list | Where-Object LanguageTag -eq 'ja')) {
  $list.Add('ja-JP')
}
$ja = $list | Where-Object { $_.LanguageTag -like 'ja*' } | Select-Object -First 1
$tipString = "0411:$tip$profileGuid"
if ($ja.InputMethodTips -notcontains $tipString) { $ja.InputMethodTips.Add($tipString) }
Set-WinUserLanguageList $list -Force
Get-WinUserLanguageList | ForEach-Object { Stage "language $($_.LanguageTag): $($_.InputMethodTips -join ', ')" }

# --- 3. the app, its field, the input language -------------------------------
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Ime {
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT {
    public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT {
    public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)] public struct UNION {
    [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public UNION u; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
    public uint cbSize; public uint flags; public IntPtr hwndActive; public IntPtr hwndFocus;
    public IntPtr hwndCapture; public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize;
    public IntPtr hwndCaret; public RECT rcCaret; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr LoadKeyboardLayoutW(string id, uint flags);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint thread);
  [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint code, uint type);
  [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int vk);
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint thread, ref GUITHREADINFO info);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
  [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr d);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);

  static INPUT KeyInput(ushort vk, bool up) {
    var i = new INPUT { type = 1 };
    i.u.ki = new KEYBDINPUT { wVk = vk, wScan = (ushort)MapVirtualKeyW(vk, 0), dwFlags = up ? 2u : 0u };
    return i;
  }
  static uint Send(params INPUT[] inputs) {
    return SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
  }
  public static uint Key(ushort vk) { return Send(KeyInput(vk, false), KeyInput(vk, true)); }

  public static string Describe(IntPtr h) {
    if (h == IntPtr.Zero) return "none";
    var t = new StringBuilder(256); GetWindowTextW(h, t, 256);
    var c = new StringBuilder(256); GetClassNameW(h, c, 256);
    uint pid; GetWindowThreadProcessId(h, out pid);
    return String.Format("0x{0:x} '{1}' ({2}, pid {3})", h.ToInt64(), t, c, pid);
  }
  public static GUITHREADINFO Gui(uint thread) {
    var g = new GUITHREADINFO(); g.cbSize = (uint)Marshal.SizeOf(typeof(GUITHREADINFO));
    GetGUIThreadInfo(thread, ref g);
    return g;
  }
  // The input desktop can be opened: it isn't locked (Winlogon's) or gone.
  public static bool InputDesktop() {
    IntPtr d = OpenInputDesktop(0, false, 0x0100); // DESKTOP_SWITCHDESKTOP
    if (d == IntPtr.Zero) return false;
    CloseDesktop(d); return true;
  }
  // SetForegroundWindow from a thread that shares the foreground thread's
  // input state, which the foreground lock lets through.
  public static bool Attached(IntPtr h) {
    uint me = GetCurrentThreadId();
    uint fg = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
    bool att = fg != 0 && fg != me && AttachThreadInput(me, fg, true);
    BringWindowToTop(h);
    bool ok = SetForegroundWindow(h);
    if (att) AttachThreadInput(me, fg, false);
    return ok;
  }
  // SetForegroundWindow while Alt is held (the lock lets a menu activation
  // through). An unassigned key (VK 0xE8) goes between the press and the
  // release: a lone Alt release would put whichever window gets it in menu
  // mode (DefWindowProc's SC_KEYMENU), and that eats the keys after it.
  public static bool WithAlt(IntPtr h) {
    Send(KeyInput(0x12, false));
    bool ok = SetForegroundWindow(h);
    ReleaseAlt();
    return ok;
  }
  public static void ReleaseAlt() {
    Send(KeyInput(0xE8, false), KeyInput(0xE8, true), KeyInput(0x12, true));
  }
  // A click on the title bar: the system activates the window it lands on.
  public static void ClickCaption(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    SetCursorPos((r.Left + r.Right) / 2, r.Top + 12);
    var down = new INPUT { type = 0 }; down.u.mi = new MOUSEINPUT { dwFlags = 2 }; // LEFTDOWN
    var up = new INPUT { type = 0 }; up.u.mi = new MOUSEINPUT { dwFlags = 4 };     // LEFTUP
    Send(down, up);
  }
}
"@

$log = Join-Path $Out 'ime-windows.log'
$err = Join-Path $Out 'ime-windows.err.log'
$env:STIPPLE_TEXTINPUT_ECHO = '1'
$env:STIPPLE_DEBUG_TSF = '1'

# The app's output: its field (stdout) and its TSF trace (stderr). Read
# while the app holds them open.
function Read-Shared($path) {
  try {
    $fs = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    $r = New-Object IO.StreamReader($fs, [Text.Encoding]::UTF8)
    $t = $r.ReadToEnd(); $r.Close(); return $t
  } catch { return '' }
}
function Field-Log { Read-Shared $log }
function Tsf-Log { Read-Shared $err }
# A mark in both logs, and what each gained since.
function Mark { @((Field-Log).Length, (Tsf-Log).Length) }
function Since($m) {
  $f = Field-Log; $t = Tsf-Log
  @($f.Substring([Math]::Min($m[0], $f.Length)), $t.Substring([Math]::Min($m[1], $t.Length)))
}
# Waits up to $Ms for $Text to show up in the field log (0) or the TSF
# trace (1) after mark $m.
function Wait-For($m, [int]$Which, [string]$Text, [int]$Ms = 3000) {
  $deadline = [DateTime]::Now.AddMilliseconds($Ms)
  do {
    if ((Since $m)[$Which].Contains($Text)) { return $true }
    Start-Sleep -Milliseconds 100
  } while ([DateTime]::Now -lt $deadline)
  return $false
}
# The field's last echoed state.
function Last-Field {
  $lines = (Field-Log) -split "`r?`n" | Where-Object { $_.StartsWith('textinput: ') }
  if ($lines) { return @($lines)[-1] } else { return '' }
}
$empty = 'textinput: text="" preedit=None'

$proc = Start-Process -FilePath $App -PassThru -RedirectStandardOutput $log -RedirectStandardError $err

function Finish([int]$Code) {
  Stop-Process -Id $proc.Id -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 500
  Write-Host "=== ime-windows.log ==="
  Get-Content $log -Encoding UTF8
  Write-Host "=== ime-windows.err.log ==="
  Get-Content $err -Encoding UTF8 -ErrorAction SilentlyContinue
  exit $Code
}

$hwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 60 -and $hwnd -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 250
  $hwnd = [Ime]::FindWindowW([NullString]::Value, $title)
}
if ($hwnd -eq [IntPtr]::Zero) { Stage "FAILED at window: no window titled '$title'"; Finish 1 }
$thread = [Ime]::GetWindowThreadProcessId($hwnd, [IntPtr]::Zero)
Stage ("window {0}, thread {1}" -f [Ime]::Describe($hwnd), $thread)
Start-Sleep -Seconds 1

# The window's input state, for the log.
function Input-State {
  $g = [Ime]::Gui($thread)
  $alt = ([Ime]::GetAsyncKeyState(0x12) -band 0x8000) -ne 0
  "foreground {0}; the app's active {1:x}, focus {2:x}, flags 0x{3:x}{4}; Alt held: {5}; input desktop: {6}" -f `
    [Ime]::Describe([Ime]::GetForegroundWindow()), [int64]$g.hwndActive, [int64]$g.hwndFocus, $g.flags, `
    $(if ($g.flags -band 0x1C) { ' (menu mode)' } else { '' }), $alt, [Ime]::InputDesktop()
}
function Fail([string]$At, [string]$Why) {
  Stage "FAILED at $At`: $Why"
  Stage "  $(Input-State)"
  Finish 1
}

# The window is in the foreground, not in menu mode, with no Alt held:
# keys sent now reach it. Each way in is tried only if the one before
# didn't do it, and the log says which one did.
function Front([string]$Why) {
  for ($try = 0; $try -lt 3; $try++) {
    $g = [Ime]::Gui($thread)
    if ($g.flags -band 0x1C) {
      # GUI_INMENUMODE | GUI_SYSTEMMENUMODE | GUI_POPUPMENUMODE: Escape
      # leaves it (or closes the menu, then leaves it).
      Stage "$Why`: the window is in menu mode (flags 0x$('{0:x}' -f $g.flags)); Escape"
      [Ime]::Key(0x1B) | Out-Null
      Start-Sleep -Milliseconds 300
      continue
    }
    if (([Ime]::GetAsyncKeyState(0x12) -band 0x8000) -ne 0) {
      Stage "$Why`: Alt is held; releasing it"
      [Ime]::ReleaseAlt()
      Start-Sleep -Milliseconds 300
      continue
    }
    if ([Ime]::GetForegroundWindow() -eq $hwnd) { return $true }
    Stage "$Why`: the foreground is $([Ime]::Describe([Ime]::GetForegroundWindow()))"
    $how = $null
    if ([Ime]::SetForegroundWindow($hwnd) -and [Ime]::GetForegroundWindow() -eq $hwnd) { $how = 'SetForegroundWindow' }
    elseif ([Ime]::Attached($hwnd) -and [Ime]::GetForegroundWindow() -eq $hwnd) { $how = 'SetForegroundWindow with AttachThreadInput' }
    elseif ([Ime]::WithAlt($hwnd) -and [Ime]::GetForegroundWindow() -eq $hwnd) { $how = 'SetForegroundWindow with Alt held' }
    else {
      [Ime]::ClickCaption($hwnd)
      Start-Sleep -Milliseconds 300
      if ([Ime]::GetForegroundWindow() -eq $hwnd) { $how = 'a click on the title bar' }
    }
    Start-Sleep -Milliseconds 300
    if ($how) { Stage "$Why`: in the foreground by $how" }
  }
  $g = [Ime]::Gui($thread)
  return ([Ime]::GetForegroundWindow() -eq $hwnd) -and -not ($g.flags -band 0x1C)
}

function Send-Keys([uint16[]]$Keys, [int]$Delay = 150) {
  foreach ($k in $Keys) {
    $n = [Ime]::Key($k)
    if ($n -ne 2) { Stage "SendInput sent $n of 2 events for VK $k (error $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))" }
    Start-Sleep -Milliseconds $Delay
  }
}

function Shot($name) {
  & powershell -NoProfile -ExecutionPolicy Bypass -File .github/scripts/post-key.ps1 -Title $title `
    -Screenshot (Join-Path $Out $name) | Out-Null
}

Stage "before any key: $(Input-State)"

# Tab focuses the field; the app's TSF trace says "focus" when TSF ties
# the window to the field's document. A Tab that didn't land (the window
# wasn't taking keys) is sent again.
$focused = $false
for ($try = 1; $try -le 3 -and -not $focused; $try++) {
  if (-not (Front 'focus the field')) { Stage "attempt $try`: can't bring the window forward"; continue }
  $m = Mark
  Send-Keys @(0x09)
  $focused = Wait-For $m 1 'stipple-tsf: focus ' 3000
  if (-not $focused) { Stage "attempt $try`: Tab didn't focus the field: $(Input-State)" }
}
if (-not $focused) {
  if ((Tsf-Log).Contains('stipple-tsf: off')) { Fail 'field focus' "TSF is off in the app: $((Tsf-Log).Trim())" }
  Fail 'field focus' "no 'stipple-tsf: focus' in the app's trace after three Tabs"
}
Stage "the field has focus (TSF: $(((Tsf-Log) -split "`r?`n" | Where-Object { $_ -like 'stipple-tsf: active*' }) -join ''))"

# WM_INPUTLANGCHANGEREQUEST: the window's thread switches, and TSF
# activates the language's default text service.
Stage ("input language before: 0x{0:x}" -f [int64][Ime]::GetKeyboardLayout($thread))
$hkl = [Ime]::LoadKeyboardLayoutW('00000411', 1)   # KLF_ACTIVATE
$lang = 0
for ($try = 1; $try -le 2 -and ($lang -band 0xFFFF) -ne 0x411; $try++) {
  [Ime]::PostMessageW($hwnd, 0x0050, [IntPtr]0, $hkl) | Out-Null
  for ($i = 0; $i -lt 30 -and ($lang -band 0xFFFF) -ne 0x411; $i++) {
    Start-Sleep -Milliseconds 100
    $lang = [int64][Ime]::GetKeyboardLayout($thread)
  }
}
Stage ("input language after: 0x{0:x}" -f $lang)
if (($lang -band 0xFFFF) -ne 0x411) { Fail 'input language' "the app's thread didn't switch to Japanese" }
Start-Sleep -Seconds 1

# --- 4. type ------------------------------------------------------------------
# Escape until the field is empty with no composition: the IME's prediction
# list, when it is up, takes the first Escape.
function Clear-Composition([string]$Why) {
  for ($i = 1; $i -le 3; $i++) {
    if ((Last-Field) -eq $empty) { return $true }
    if ((Last-Field).Contains('preedit=Some(')) {
      $m = Mark
      Send-Keys @(0x1B)
      if (Wait-For $m 0 $empty 1500) {
        if ($i -gt 1) { Stage "$Why`: Escape $i cancelled the composition" }
        return $true
      }
      Stage "$Why`: Escape $i left $(Last-Field)"
    } else {
      # Text typed with the IME off, or a stray commit: Backspace it.
      Stage "$Why`: the field has $(Last-Field); Backspace"
      $m = Mark
      Send-Keys @(0x08)
      Wait-For $m 0 'textinput: ' 1500 | Out-Null
    }
  }
  return (Last-Field) -eq $empty
}

# Turn the IME on: VK_IME_ON, or failing that VK_KANJI (the hankaku/zenkaku
# toggle). "a" must then become a preedit, which Escape cancels; a plain
# "a" that went into the field is taken back with Backspace.
$on = $false
for ($round = 1; $round -le 2 -and -not $on; $round++) {
  foreach ($toggle in @(0x16, 0x19)) {
    if (-not (Front 'turn the IME on')) { Fail 'foreground' 'the window left the foreground' }
    Send-Keys @($toggle)
    Start-Sleep -Milliseconds 500
    $m = Mark
    Send-Keys @(0x41)        # a
    if (Wait-For $m 0 'preedit=Some(' 1500) {
      Stage ("VK 0x{0:x} turned the IME on" -f $toggle)
      $on = $true
    } elseif ((Since $m)[0].Contains('textinput: ')) {
      Stage ("VK 0x{0:x} left the IME off ('a' typed {1})" -f $toggle, (Last-Field))
    } else {
      Stage ("VK 0x{0:x}: 'a' changed nothing: {1}" -f $toggle, (Input-State))
    }
    if (-not (Clear-Composition 'after the probe')) { Fail 'cancel' "the field didn't empty: $(Last-Field)" }
    if ($on) { break }
  }
}
if (-not $on) { Fail 'IME on' 'neither VK_IME_ON nor VK_KANJI turned the IME on' }
Start-Sleep -Milliseconds 500

$nihoKana = [string][char]0x306B + [char]0x307B   # ni ho (hiragana)
$nihonKanji = [string][char]0x65E5 + [char]0x672C # nihon (kanji)
$wantPreedit = "textinput: text=`"`" preedit=Some(`"$nihoKana"
$wantConverted = "textinput: text=`"`" preedit=Some(`"$nihonKanji`")"
$wantCommit = "textinput: text=`"$nihonKanji`" preedit=None"
# (Spelled with code points: Windows PowerShell reads a script without a BOM
# as ANSI.)

$done = $false
for ($attempt = 1; $attempt -le 2 -and -not $done; $attempt++) {
  if ($attempt -gt 1) {
    if (-not (Clear-Composition 'before retyping')) { Fail 'cancel' "the field didn't empty: $(Last-Field)" }
    Stage "retyping (attempt $attempt)"
  }
  if (-not (Front 'type')) { Fail 'foreground' 'the window left the foreground' }
  $m = Mark
  Send-Keys @(0x4E, 0x49, 0x48, 0x4F, 0x4E)   # n i h o n
  if (-not (Wait-For $m 0 $wantPreedit 3000)) {
    Stage "attempt $attempt`: no hiragana preedit; the field: $(Last-Field); $(Input-State)"
    continue
  }
  Start-Sleep -Milliseconds 500
  Shot 'stipple-windows-ime-preedit.png'
  $m = Mark
  Send-Keys @(0x20)          # Space: convert
  if (-not (Wait-For $m 0 $wantConverted 3000)) {
    Stage "attempt $attempt`: Space didn't convert to the expected kanji; the field: $(Last-Field)"
    continue
  }
  Start-Sleep -Milliseconds 500
  Shot 'stipple-windows-ime-converted.png'
  $m = Mark
  Send-Keys @(0x0D)          # Return: commit
  if (-not (Wait-For $m 0 $wantCommit 3000)) {
    Stage "attempt $attempt`: Return didn't commit; the field: $(Last-Field)"
    continue
  }
  $done = $true
}
Send-Keys @(0x1A)          # VK_IME_OFF
Start-Sleep -Milliseconds 500

$text = Field-Log
foreach ($want in @($wantPreedit, $wantConverted, $wantCommit)) {
  if ($text.Contains($want)) { Stage "ok: $want" } else { Stage "missing: $want" }
}
if (-not $done) {
  if (-not (Tsf-Log).Contains('composition started')) {
    Fail 'compose' 'the IME started no composition (keys didn''t reach it, or it stayed in direct input)'
  }
  Fail 'compose' "the word didn't go through preedit, conversion and commit; the field: $(Last-Field)"
}
Stage "ok"
Finish 0
