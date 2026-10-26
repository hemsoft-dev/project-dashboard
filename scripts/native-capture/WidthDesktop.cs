using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

// Additional APIs for issue 27 only. Display changes are dynamic, never saved to the registry.
// https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-changedisplaysettingsexw
public static class WidthDesktop {
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    public struct Mode {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Device;
        public ushort SpecVersion, DriverVersion, Size, DriverExtra;
        public uint Fields;
        public int X, Y;
        public uint Orientation, FixedOutput;
        public short Color, Duplex, YResolution, TTOption, Collate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Form;
        public ushort LogPixels;
        public uint BitsPerPixel, Width, Height, Flags, Frequency;
        public uint ICMMethod, ICMIntent, MediaType, DitherType, Reserved1, Reserved2, PanningWidth, PanningHeight;
    }
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool EnumDisplaySettingsW(string device, int number, ref Mode mode);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    public static extern int ChangeDisplaySettingsExW(string device, ref Mode mode, IntPtr window, uint flags, IntPtr parameter);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll", SetLastError=true)] static extern bool MoveWindow(IntPtr window, int x, int y, int width, int height, bool paint);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SystemParametersInfoW(uint action, uint parameter, out Rect rect, uint flags);
    public static Rect WorkArea() {
        Rect rect;
        if (!SystemParametersInfoW(0x0030, 0, out rect, 0)) throw new Win32Exception(Marshal.GetLastWin32Error(), "SPI_GETWORKAREA failed");
        return rect;
    }
    public static Mode ReadMode(int number) {
        var mode = new Mode { Size=(ushort)Marshal.SizeOf(typeof(Mode)) };
        if (!EnumDisplaySettingsW(null, number, ref mode)) throw new Win32Exception(Marshal.GetLastWin32Error(), "EnumDisplaySettingsW failed");
        return mode;
    }
    public static Mode DesiredMode() {
        // Prefer a driver-enumerated mode as documented. If absent, test the current mode
        // with width/height fields only; the returned DISP_CHANGE code records rejection.
        for (int i=0; i<512; i++) {
            Mode mode;
            try { mode=ReadMode(i); } catch (Win32Exception) { break; }
            if (mode.Width==1920 && mode.Height==1080 && mode.BitsPerPixel==32) return mode;
        }
        var desired=ReadMode(-1);
        desired.Width=1920; desired.Height=1080; desired.Fields=0x00080000 | 0x00100000;
        return desired;
    }
    public static Rect Client(IntPtr window) {
        Rect rect;
        if (!GetClientRect(window, out rect)) throw new Win32Exception(Marshal.GetLastWin32Error(), "GetClientRect failed");
        return rect;
    }
    public static void ResizeClient(IntPtr window, int x, int y, int width, int height) {
        Rect outer;
        if (!GetWindowRect(window, out outer)) throw new Win32Exception(Marshal.GetLastWin32Error(), "GetWindowRect failed");
        var client=Client(window);
        int borderWidth=outer.Right-outer.Left-(client.Right-client.Left);
        int borderHeight=outer.Bottom-outer.Top-(client.Bottom-client.Top);
        if (!MoveWindow(window, x, y, width+borderWidth, height+borderHeight, true))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "MoveWindow failed");
    }
}
