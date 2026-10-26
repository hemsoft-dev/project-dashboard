using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

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
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr GetProcessWindowStation();
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool GetUserObjectInformationW(IntPtr handle, int index, StringBuilder name, int bytes, out int needed);
    public static string WindowStationName() {
        var station=GetProcessWindowStation();
        var name=new StringBuilder(256); int needed;
        if (station==IntPtr.Zero || !GetUserObjectInformationW(station, 2, name, 512, out needed))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot identify process window station");
        return name.ToString();
    }
    public static int ModeSize() { return Marshal.SizeOf(typeof(Mode)); }
    public static Mode ChangeRequest(Mode source) {
        // This marshaller supplies the public DEVMODEW only, with no private driver tail.
        // dmDriverExtra counts bytes FOLLOWING that structure; dmFields names requested members.
        // https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-devmodew
        // Use only documented display fields. Keep frequency to select/restore the same mode.
        // https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-changedisplaysettingsexw
        // Mode is a value type: normalize a copy, preserving the driver-returned version/data.
        source.Size=(ushort)ModeSize();
        source.DriverExtra=0;
        source.Fields=0x00040000 | 0x00080000 | 0x00100000 | 0x00400000;
        return source;
    }
    public static Rect WorkArea() {
        Rect rect;
        if (!SystemParametersInfoW(0x0030, 0, out rect, 0)) throw new Win32Exception(Marshal.GetLastWin32Error(), "SPI_GETWORKAREA failed");
        return rect;
    }
    public static Mode ReadMode(int number) {
        var mode = new Mode { Size=(ushort)Marshal.SizeOf(typeof(Mode)), DriverExtra=0 };
        if (!EnumDisplaySettingsW(null, number, ref mode)) throw new Win32Exception(Marshal.GetLastWin32Error(), "EnumDisplaySettingsW failed");
        return mode;
    }
    public struct EnumeratedMode { public int Index; public Mode Settings; }
    public static EnumeratedMode[] EnumerateModes() {
        var modes=new List<EnumeratedMode>();
        // Enumerate every index until the documented zero return, without inventing a mode.
        for (int i=0; ; i++) {
            var mode=new Mode { Size=(ushort)Marshal.SizeOf(typeof(Mode)), DriverExtra=0 };
            if (!EnumDisplaySettingsW(null, i, ref mode)) break;
            modes.Add(new EnumeratedMode { Index=i, Settings=mode });
        }
        return modes.ToArray();
    }
    public static EnumeratedMode SelectWidest(EnumeratedMode[] modes) {
        if (modes==null || modes.Length==0) throw new InvalidOperationException("EnumDisplaySettingsW returned no display modes.");
        var best=modes[0];
        foreach (var candidate in modes) {
            var mode=candidate.Settings;
            var current=best.Settings;
            if (mode.Width>current.Width ||
                (mode.Width==current.Width && mode.BitsPerPixel==32 && current.BitsPerPixel!=32) ||
                (mode.Width==current.Width && (mode.BitsPerPixel==32)==(current.BitsPerPixel==32) && mode.Height>current.Height))
                best=candidate;
        }
        // Keep the complete driver-returned DEVMODE, including flags/frequency/position.
        return best;
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
