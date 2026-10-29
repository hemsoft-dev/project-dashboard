param([Parameter(Mandatory)][string]$Output, [ValidateSet('Read','Normal','Maximized')][string]$Mode='Read')
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W53 {
 [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Ri,B; }
 [StructLayout(LayoutKind.Sequential)] public struct P { public int X,Y; }
 [StructLayout(LayoutKind.Sequential)] public struct WP { public int Length,Flags,Show; public P Min,Max; public R Rect; }
 [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr h,ref WP p);
 [DllImport("user32.dll")] public static extern bool SetWindowPlacement(IntPtr h,ref WP p);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out R r);
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int command);
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] public struct MI { public int Size; public R Monitor,Work; public int Flags; [MarshalAs(UnmanagedType.ByValTStr,SizeConst=32)] public string Device; }
 [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h,int flags);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern bool GetMonitorInfoW(IntPtr h,ref MI info);
 [DllImport("user32.dll")] public static extern uint GetDpiForSystem();
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
}
'@
[void][W53]::SetThreadDpiAwarenessContext([IntPtr](-4))
$p=Get-Process project-dashboard | Where-Object SessionId -eq (Get-Process -Id $PID).SessionId | Select-Object -First 1
$h=$p.MainWindowHandle
if (!$h) { throw 'No Dashboard window in interactive session' }
if ($Mode -ne 'Read') { $command=if($Mode -eq 'Normal'){9}else{3}; [void][W53]::ShowWindow($h,$command); Start-Sleep -Seconds 1 }
$w=New-Object W53+WP; $w.Length=44
if (![W53]::GetWindowPlacement($h,[ref]$w)) {throw 'GetWindowPlacement failed'}
$r=New-Object W53+R; if (![W53]::GetWindowRect($h,[ref]$r)) {throw 'GetWindowRect failed'}
$m=New-Object W53+MI; $m.Size=[Runtime.InteropServices.Marshal]::SizeOf($m)
if (![W53]::GetMonitorInfoW([W53]::MonitorFromWindow($h,2),[ref]$m)) { throw 'GetMonitorInfoW failed' }
@{systemDpi=[W53]::GetDpiForSystem();placement=$w;visible=$r;dpi=[W53]::GetDpiForWindow($h);monitor=$m.Device} | ConvertTo-Json -Depth 5 | Set-Content $Output
Add-Type -AssemblyName System.Drawing
$bitmap=New-Object Drawing.Bitmap ($r.Ri-$r.L),($r.B-$r.T)
$graphics=[Drawing.Graphics]::FromImage($bitmap)
try {
 $dc=$graphics.GetHdc()
 try { if (![W53]::PrintWindow($h,$dc,2)) {throw 'PrintWindow failed'} } finally {$graphics.ReleaseHdc($dc)}
 $bitmap.Save($Output+'.png')
} finally { $graphics.Dispose(); $bitmap.Dispose() }
