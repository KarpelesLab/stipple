# Drive menudemo's native menus on Windows and check what ran from its log:
# read the HMENU's structure back, pick from the menu bar with WM_COMMAND and
# with the keyboard (Alt+E, then a mnemonic), use a shortcut, and open the
# native context menu (a "#32768" popup window) and pick from it.
param(
  [string]$Exe = ".\target\release\menudemo.exe",
  [string]$Out = "visual_output"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$log = Join-Path $Out "menu-windows.log"
$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
  -RedirectStandardError (Join-Path $Out "menu-windows.err")
Start-Sleep -Seconds 5

Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class M {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr m);
  [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr m, int pos);
  [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr m, int pos);
  [DllImport("user32.dll")] public static extern uint GetMenuState(IntPtr m, uint item, uint flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetMenuStringW(IntPtr m, uint item, StringBuilder s, int max, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
}
"@

$MF_BYPOSITION = 0x400
function Label($menu, $i) {
  $sb = New-Object System.Text.StringBuilder 256
  [M]::GetMenuStringW($menu, [uint32]$i, $sb, 256, $MF_BYPOSITION) | Out-Null
  $sb.ToString()
}
function LogLines($pattern) {
  @(Get-Content $log -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern }).Count
}
function WaitFor($pattern, $count) {
  for ($i = 0; $i -lt 20 -and (LogLines $pattern) -lt $count; $i++) { Start-Sleep -Milliseconds 250 }
}
function Screenshot($path) {
  try {
    Add-Type -AssemblyName System.Drawing
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
    $gfx = [System.Drawing.Graphics]::FromImage($bmp)
    $gfx.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $PWD $path))
  } catch { Write-Host "screenshot failed: $_" }
}
$failures = @()
function Check($ok, $what) {
  if ($ok) { Write-Host "ok: $what" } else { Write-Host "FAILED: $what"; $script:failures += $what }
}

try {
  $hwnd = [M]::FindWindowW([NullString]::Value, "Stipple Menus")
  if ($hwnd -eq [IntPtr]::Zero) { throw "no 'Stipple Menus' window" }
  [M]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Seconds 1

  # The menu bar's structure, as Windows holds it.
  $bar = [M]::GetMenu($hwnd)
  Check ($bar -ne [IntPtr]::Zero) "the window has a menu bar"
  $titles = @()
  $menus = @{}
  for ($i = 0; $i -lt [M]::GetMenuItemCount($bar); $i++) {
    $title = Label $bar $i
    $titles += $title
    $sub = [M]::GetSubMenu($bar, $i)
    $items = @()
    for ($j = 0; $j -lt [M]::GetMenuItemCount($sub); $j++) {
      $state = [M]::GetMenuState($sub, [uint32]$j, $MF_BYPOSITION)
      $text = Label $sub $j
      if ($state -band 0x800) { $text = "-" }
      if ($state -band 0x3) { $text += " (grayed)" }
      if ($state -band 0x8) { $text += " (checked)" }
      $items += $text
    }
    $menus[$title] = @{ handle = $sub; items = $items }
    Write-Host "menu $title : $($items -join ' | ')"
  }
  Check (($titles -join ",") -eq "&File,&Edit,&View,&Help") "top-level menus with mnemonics: $($titles -join ',')"
  Check ($menus["&File"].items[0] -eq "&New`tCtrl+N") "File > New shows its mnemonic and shortcut"
  Check ($menus["&File"].items[1] -like "&Save*(grayed)") "File > Save is grayed with nothing to save"
  Check ($menus["&View"].items[0] -like "Show &Grid*(checked)") "View > Show Grid is checked"

  # WM_COMMAND with Edit > Add Item's id, as a click on it sends.
  $add = [M]::GetMenuItemID($menus["&Edit"].handle, 0)
  [M]::PostMessageW($hwnd, 0x0111, [IntPtr]$add, [IntPtr]::Zero) | Out-Null
  WaitFor "^menudemo: Added item 1 " 1
  Check ((LogLines "^menudemo: Added item 1 ") -eq 1) "WM_COMMAND $add ran Edit > Add Item"

  # The keyboard: Alt+E opens Edit, then its mnemonic picks Add Item.
  [M]::SetForegroundWindow($hwnd) | Out-Null
  [System.Windows.Forms.SendKeys]::SendWait("%e")
  Start-Sleep -Milliseconds 700
  Screenshot (Join-Path $Out "menu-windows-edit.png")
  [System.Windows.Forms.SendKeys]::SendWait("a")
  WaitFor "^menudemo: Added item 2 " 1
  Check ((LogLines "^menudemo: Added item 2 ") -eq 1) "Alt+E, A picked Edit > Add Item"

  # The shortcut runs it exactly once.
  [System.Windows.Forms.SendKeys]::SendWait("^d")
  WaitFor "^menudemo: Added item 3 " 1
  Start-Sleep -Milliseconds 500
  Check ((LogLines "^menudemo: Added item 3 ") -eq 1) "Ctrl+D ran Add Item"
  Check ((LogLines "^menudemo: Added item 4 ") -eq 0) "...once"

  # A right-click on the canvas opens the native context menu.
  $scale = [M]::GetDpiForWindow($hwnd) / 96.0
  $x = [int](320 * $scale); $y = [int](300 * $scale)
  [M]::PostMessageW($hwnd, 0x0204, [IntPtr]2, [IntPtr](($y -shl 16) -bor $x)) | Out-Null
  $popup = [IntPtr]::Zero
  for ($i = 0; $i -lt 20 -and $popup -eq [IntPtr]::Zero; $i++) {
    Start-Sleep -Milliseconds 250
    $popup = [M]::FindWindowW("#32768", [NullString]::Value)
  }
  Check ($popup -ne [IntPtr]::Zero) "a native context menu (#32768) opened"
  Screenshot (Join-Path $Out "menu-windows-context.png")
  # Its "&Add Item" mnemonic picks it.
  [System.Windows.Forms.SendKeys]::SendWait("a")
  WaitFor "^menudemo: Added item 4 " 1
  Check ((LogLines "^menudemo: Added item 4 ") -eq 1) "the context menu's mnemonic picked Add Item"
  Start-Sleep -Milliseconds 500
  Check ([M]::FindWindowW("#32768", [NullString]::Value) -eq [IntPtr]::Zero) "the context menu closed"
}
finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Write-Host "=== menu-windows.log ==="
  Get-Content $log -ErrorAction SilentlyContinue
  Get-Content (Join-Path $Out "menu-windows.err") -ErrorAction SilentlyContinue | Select-Object -Last 20
}
if ($failures.Count -gt 0) { throw "menu checks failed: $($failures -join '; ')" }
