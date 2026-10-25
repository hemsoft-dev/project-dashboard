using System;
using System.Runtime.InteropServices;
using System.Text;

// Only the owned Dashboard window may receive input or be captured.
public static class NativeDesktop {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSE { public int X,Y; public uint Data,Flags,Time; public UIntPtr Extra; }
    [StructLayout(LayoutKind.Sequential)] public struct KEY { public ushort Vk,Scan; public uint Flags,Time; public UIntPtr Extra; }
    [StructLayout(LayoutKind.Explicit)] public struct UNION { [FieldOffset(0)] public MOUSE Mouse; [FieldOffset(0)] public KEY Key; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint Type; public UNION Data; }
    [DllImport("user32.dll",SetLastError=true)] public static extern bool GetWindowRect(IntPtr h,out RECT r);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll",SetLastError=true)] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int command);
    [DllImport("user32.dll",SetLastError=true)] public static extern bool MoveWindow(IntPtr h,int x,int y,int w,int height,bool paint);
    [DllImport("user32.dll",SetLastError=true)] public static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll",SetLastError=true)] public static extern uint SendInput(uint count,INPUT[] input,int size);
    [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr h);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern bool GetUserObjectInformation(IntPtr h,int index,StringBuilder name,int size,out int needed);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    static void Send(INPUT[] events) {
        if (SendInput((uint)events.Length,events,Marshal.SizeOf(typeof(INPUT))) != events.Length)
            throw new InvalidOperationException("Interactive input unavailable: SendInput did not deliver every event.");
    }
    static INPUT Key(ushort key,uint flags) { return new INPUT {Type=1,Data=new UNION{Key=new KEY{Vk=key,Flags=flags}}}; }
    public static void F5() { Send(new[]{Key(0x74,0),Key(0x74,2)}); }
    public static void Click(int x,int y) {
        if (!SetCursorPos(x,y)) throw new InvalidOperationException("Interactive input unavailable: SetCursorPos failed.");
        Send(new[]{new INPUT{Data=new UNION{Mouse=new MOUSE{Flags=2}}},new INPUT{Data=new UNION{Mouse=new MOUSE{Flags=4}}}});
    }
    public static string InputDesktop() {
        var h=OpenInputDesktop(0,false,1);
        if(h==IntPtr.Zero) throw new InvalidOperationException("Locked/noninteractive session: OpenInputDesktop failed.");
        try { var name=new StringBuilder(256); int needed;
            if(!GetUserObjectInformation(h,2,name,512,out needed)) throw new InvalidOperationException("Cannot identify input desktop.");
            return name.ToString();
        } finally { CloseDesktop(h); }
    }
}
