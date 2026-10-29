# Capture a specific top-level window without moving the mouse.
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Drawing;
using System.Drawing.Imaging;
public class YkExplorerCapture {
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
 public static bool Save(IntPtr h, string path) {
   RECT r; if (!GetWindowRect(h, out r)) return false;
   int w = r.R - r.L, ht = r.B - r.T;
   if (w <= 0 || ht <= 0) return false;
   using (var bmp = new Bitmap(w, ht)) {
     using (var g = Graphics.FromImage(bmp)) {
       IntPtr dc = g.GetHdc();
       bool ok = PrintWindow(h, dc, 2);
       g.ReleaseHdc(dc);
       bmp.Save(path, ImageFormat.Png);
       return ok;
     }
   }
 }
}
'@ -ReferencedAssemblies System.Drawing
function Save-WindowPng {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle, [Parameter(Mandatory = $true)][string]$Path)
    [YkExplorerCapture]::Save($Handle, $Path)
}
