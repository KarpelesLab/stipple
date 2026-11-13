# Type Japanese into examples/textinput through the real Microsoft Japanese
# IME, by way of TSF.
#
#   .github\scripts\ime-windows.ps1 <textinput.exe> [out dir]
#
# 1. The IME: the Japanese language's typing feature carries it. If its TSF
#    profile isn't registered, try to add it (Add-WindowsCapability, then
#    Install-Language), with Windows Update started if the image stopped it.
# 2. The user's language list gets ja-JP with the IME as its input method.
# 3. The app starts, Tab focuses the field (TSF ties the window to its
#    document), and WM_INPUTLANGCHANGEREQUEST switches the window's thread to
#    Japanese, which activates the IME's profile.
# 4. SendInput: VK_IME_ON (or VK_KANJI) until "a" becomes a preedit, Escape
#    cancels it, then n i h o n (a hiragana preedit), Space (the conversion,
#    nihon in kanji), Return (the commit). The app prints its field
#    (STIPPLE_TEXTINPUT_ECHO) and its TSF trace (STIPPLE_DEBUG_TSF).
#
# Each stage logs what it found, so a run that can't get the IME says where
# it stopped. Exit code 0 only when the commit arrived.
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
    Stage "FAILED: no Japanese IME could be installed on this runner"
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
using System.Runtime.InteropServices;
public static class Ime {
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT {
    public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT {
    public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)] public struct UNION {
    [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public UNION u; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr LoadKeyboardLayoutW(string id, uint flags);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint thread);
  [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint code, uint type);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
  public static uint Key(ushort vk) {
    var down = new INPUT { type = 1 };
    down.u.ki = new KEYBDINPUT { wVk = vk, wScan = (ushort)MapVirtualKeyW(vk, 0) };
    var up = down;
    up.u.ki.dwFlags = 2; // KEYEVENTF_KEYUP
    return SendInput(2, new[] { down, up }, Marshal.SizeOf(typeof(INPUT)));
  }
}
"@

$log = Join-Path $Out 'ime-windows.log'
$err = Join-Path $Out 'ime-windows.err.log'
$env:STIPPLE_TEXTINPUT_ECHO = '1'
$env:STIPPLE_DEBUG_TSF = '1'
$proc = Start-Process -FilePath $App -PassThru -RedirectStandardOutput $log -RedirectStandardError $err
$hwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 60 -and $hwnd -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 250
  $hwnd = [Ime]::FindWindowW([NullString]::Value, $title)
}
if ($hwnd -eq [IntPtr]::Zero) { Stage "FAILED: no window titled '$title'"; exit 1 }
Start-Sleep -Seconds 1

function Front {
  # A foreground change is refused to a process that has no input; an Alt
  # tap first lets it through.
  [Ime]::keybd_event(0x12, 0, 0, [IntPtr]::Zero); [Ime]::keybd_event(0x12, 0, 2, [IntPtr]::Zero)
  [Ime]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 300
  if ([Ime]::GetForegroundWindow() -ne $hwnd) { Stage "warning: the window isn't in the foreground" }
}

function Send-Keys([ushort[]]$Keys, [int]$Delay = 150) {
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

Front
Send-Keys @(0x09)          # Tab: the field takes focus
Start-Sleep -Seconds 1

$thread = [Ime]::GetWindowThreadProcessId($hwnd, [IntPtr]::Zero)
Stage ("input language before: 0x{0:x}" -f [int64][Ime]::GetKeyboardLayout($thread))
$hkl = [Ime]::LoadKeyboardLayoutW('00000411', 1)   # KLF_ACTIVATE
# WM_INPUTLANGCHANGEREQUEST: the window's thread switches, and TSF
# activates the language's default text service.
[Ime]::PostMessageW($hwnd, 0x0050, [IntPtr]0, $hkl) | Out-Null
Start-Sleep -Seconds 2
Stage ("input language after: 0x{0:x}" -f [int64][Ime]::GetKeyboardLayout($thread))

# --- 4. type ------------------------------------------------------------------
function Log-Text {
  try {
    $fs = [IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
    $r = New-Object IO.StreamReader($fs, [Text.Encoding]::UTF8)
    $t = $r.ReadToEnd(); $r.Close(); return $t
  } catch { return '' }
}

# Turn the IME on: VK_IME_ON, or failing that VK_KANJI (the hankaku/zenkaku
# toggle). "a" must then become a preedit, which Escape cancels; a plain
# "a" that went into the field is taken back with Backspace.
Front
$on = $false
foreach ($toggle in @(0x16, 0x19)) {
  Send-Keys @($toggle)
  Start-Sleep -Milliseconds 500
  $before = (Log-Text).Length
  Send-Keys @(0x41)        # a
  Start-Sleep -Milliseconds 800
  $new = (Log-Text).Substring([Math]::Min($before, (Log-Text).Length))
  if ($new.Contains('preedit=Some(')) {
    Stage ("VK 0x{0:x} turned the IME on" -f $toggle)
    Send-Keys @(0x1B)      # Escape: cancel the composition
    Start-Sleep -Milliseconds 800
    $on = $true
    break
  }
  Stage ("VK 0x{0:x} left the IME off" -f $toggle)
  Send-Keys @(0x08)        # Backspace
  Start-Sleep -Milliseconds 300
}
if (-not $on) { Stage "warning: no toggle turned the IME on; typing anyway" }
Send-Keys @(0x4E, 0x49, 0x48, 0x4F, 0x4E)   # n i h o n
Start-Sleep -Seconds 1
Shot 'stipple-windows-ime-preedit.png'
Send-Keys @(0x20)          # Space: convert
Start-Sleep -Seconds 1
Shot 'stipple-windows-ime-converted.png'
Send-Keys @(0x0D)          # Return: commit
Start-Sleep -Seconds 1
Send-Keys @(0x1A)          # VK_IME_OFF
Start-Sleep -Milliseconds 500

Stop-Process -Id $proc.Id -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Write-Host "=== ime-windows.log ==="
Get-Content $log -Encoding UTF8
Get-Content $err -Encoding UTF8 -ErrorAction SilentlyContinue

$text = Log-Text
$ok = $true
# (Spelled with code points: Windows PowerShell reads a script without a BOM
# as ANSI.)
$nihoKana = [string][char]0x306B + [char]0x307B   # ni ho (hiragana)
$nihonKanji = [string][char]0x65E5 + [char]0x672C # nihon (kanji)
$wanted = @(
  "textinput: text=`"`" preedit=Some(`"$nihoKana",
  "textinput: text=`"`" preedit=Some(`"$nihonKanji`")",
  "textinput: text=`"$nihonKanji`" preedit=None"
)
foreach ($want in $wanted) {
  if ($text.Contains($want)) { Stage "ok: $want" } else { Stage "missing: $want"; $ok = $false }
}
if (-not $ok) {
  if (-not $text.Contains('stipple-tsf: active')) { Stage "TSF didn't activate in the app" }
  elseif (-not $text.Contains('composition started')) { Stage "the IME started no composition (keys didn't reach it, or it stayed in direct input)" }
  Stage "FAILED"
  exit 1
}
Stage "ok"
