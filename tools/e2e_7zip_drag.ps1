# End-to-end check for dragging a file out of a 7-Zip archive window into
# YKExplorer. It moves the real mouse cursor: use it on a machine you can
# disturb, or run it while the desktop is idle.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\e2e_7zip_drag.ps1 `
#       -Exe target\debug\YKExplorer.exe -WorkDir target\e2e-fixed
#
# The destination must be writable by YKExplorer; a folder outside a sandboxed
# workspace is the only way to see the real result.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$WorkDir,
    [string]$LogPath = "",
    [string]$SevenZip = 'C:\Program Files\7-Zip'
)

$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class YkE2E {
 [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h,int x,int y,int w,int hh,bool repaint);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint dx,uint dy,uint d,IntPtr e);
 public const uint DOWN = 0x0002, UP = 0x0004;
 // 7-Zip item rows start at (40, 168) in a window at (0, 0); the drop lands in
 // YKExplorer's file list at (1660, 540) when its window is at (860, 40).
 public static void Drag(IntPtr source, int x1, int y1, int x2, int y2, int steps) {
   SetForegroundWindow(source); System.Threading.Thread.Sleep(600);
   SetCursorPos(x1, y1); System.Threading.Thread.Sleep(500);
   // First click focuses the archive window and selects the item; without it
   // the very first button-down can be swallowed by window activation.
   mouse_event(DOWN, 0, 0, 0, IntPtr.Zero); System.Threading.Thread.Sleep(60);
   mouse_event(UP, 0, 0, 0, IntPtr.Zero); System.Threading.Thread.Sleep(400);
   mouse_event(DOWN, 0, 0, 0, IntPtr.Zero); System.Threading.Thread.Sleep(350);
   for (int i = 1; i <= steps; i++) {
     int x = x1 + (x2 - x1) * i / steps, y = y1 + (y2 - y1) * i / steps;
     SetCursorPos(x, y); System.Threading.Thread.Sleep(25);
   }
   System.Threading.Thread.Sleep(800);
   mouse_event(UP, 0, 0, 0, IntPtr.Zero);
 }
}
'@

$work = (New-Item -ItemType Directory -Path $WorkDir -Force).FullName
$temp = Join-Path $work 'temp'
$dest = Join-Path $work 'dest'
$src = Join-Path $work 'src'
$archive = Join-Path $work 'archive.7z'
Remove-Item $dest, $temp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $temp, $dest, $src -Force | Out-Null

if (-not (Test-Path $archive)) {
    Set-Content -Path (Join-Path $src 'payload.txt') -Value ("dragged out of an archive " + (Get-Date -Format o)) -Encoding utf8
    Push-Location $src
    try {
        & (Join-Path $SevenZip '7z.exe') a -t7z $archive 'payload.txt' | Out-Null
    } finally {
        Pop-Location
    }
}

$env:TEMP = $temp
$env:TMP = $temp

$fm = Start-Process (Join-Path $SevenZip '7zFM.exe') -ArgumentList "`"$archive`"" -PassThru
Start-Sleep -Seconds 3
[YkE2E]::MoveWindow($fm.MainWindowHandle, 0, 0, 760, 640, $true) | Out-Null

if ($LogPath -ne "") {
    $app = Start-Process $Exe -ArgumentList $dest -RedirectStandardError $LogPath -RedirectStandardOutput (Join-Path $work 'out.log') -PassThru
} else {
    $app = Start-Process $Exe -ArgumentList $dest -PassThru
}
Start-Sleep -Seconds 6
$app.Refresh()
[YkE2E]::MoveWindow($app.MainWindowHandle, 860, 40, 1500, 1000, $true) | Out-Null
Start-Sleep -Seconds 1

[YkE2E]::Drag($fm.MainWindowHandle, 40, 168, 1660, 540, 45)
Start-Sleep -Seconds 5

Write-Output '=== dest ==='
Get-ChildItem $dest -Recurse -Force | ForEach-Object { $_.FullName + '  ' + $_.Length }
Write-Output '=== temp ==='
Get-ChildItem $temp -Recurse -Force | ForEach-Object { $_.FullName }

foreach ($process in @($app, $fm)) {
    try { $process.Refresh(); if (-not $process.HasExited) { $process.Kill() } } catch { }
}
