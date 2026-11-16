# Start a Stipple app, find its window through UI Automation (as Narrator
# does, from the window's handle), walk its control view and check it holds
# an element of each control type and name asked for ("Type|Name prefix",
# "*" for any name), e.g. "Tree|Folders" "DataGrid|Files" "Slider|Position".
# The generic twin of uia-walk.ps1 (which drives touchdemo in depth).
param(
  [string]$Exe,
  [string]$Title,
  [string]$Arguments = "",
  [string]$Out = "visual_output",
  [string[]]$Expect = @()
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$name = [System.IO.Path]::GetFileNameWithoutExtension($Exe)
$start = @{
  FilePath = $Exe; PassThru = $true
  RedirectStandardOutput = (Join-Path $Out "$name-uia.log")
  RedirectStandardError = (Join-Path $Out "$name-uia.err")
}
if ($Arguments) { $start.ArgumentList = $Arguments }
$proc = Start-Process @start

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class R {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$A = [System.Windows.Automation.AutomationElement]

$win = $null
for ($i = 0; $i -lt 60 -and $win -eq $null; $i++) {
  [IntPtr]$h = [R]::FindWindowW([NullString]::Value, $Title)
  $owner = [uint32]0
  if ($h -ne [IntPtr]::Zero) { [void][R]::GetWindowThreadProcessId($h, [ref]$owner) }
  if ($h -ne [IntPtr]::Zero -and $owner -eq $proc.Id) {
    try { $win = $A::FromHandle($h) } catch { Write-Host "uia-roles: FromHandle failed: $_" }
  }
  if ($win -eq $null) { Start-Sleep -Milliseconds 500 }
}
$missing = @()
try {
  if ($win -eq $null) { throw "no '$Title' window for process $($proc.Id)" }
  Start-Sleep -Seconds 3
  $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
  $seen = New-Object System.Collections.ArrayList
  $stack = New-Object System.Collections.Stack
  $stack.Push(@($win, 0))
  while ($stack.Count -gt 0) {
    $item = $stack.Pop()
    $e = $item[0]; $depth = $item[1]
    $c = $e.Current
    $type = $c.ControlType.ProgrammaticName -replace 'ControlType.', ''
    [void]$seen.Add(@($type, $c.Name))
    Write-Host ("uia-roles: " + ("  " * $depth) + "$type ""$($c.Name)"" focusable=$($c.IsKeyboardFocusable)")
    $kids = @()
    $k = $walker.GetFirstChild($e)
    while ($k -ne $null) { $kids += $k; $k = $walker.GetNextSibling($k) }
    for ($j = $kids.Count - 1; $j -ge 0; $j--) { $stack.Push(@($kids[$j], ($depth + 1))) }
  }
  foreach ($want in $Expect) {
    $type, $prefix = $want -split '\|', 2
    $hit = $seen | Where-Object { $_[0] -eq $type -and ($prefix -eq '*' -or $_[1].StartsWith($prefix)) } | Select-Object -First 1
    if ($hit -eq $null) { $missing += $want }
  }
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
if ($missing.Count -gt 0) {
  Write-Host "uia-roles: FAILED: missing $($missing -join ', ')"
  exit 1
}
Write-Host "uia-roles: ok ($($seen.Count) elements)"
