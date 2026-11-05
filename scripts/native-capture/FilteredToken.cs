using System;
using System.Collections;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

// Starts a process the way UAC starts a non-elevated one from an administrator's logon: Administrators held
// deny-only, no extra privileges, medium integrity. Such a token can't name Administrators as a file's owner.
public static class FilteredToken {
    const string Administrators = "S-1-5-32-544";
    const string MediumIntegrity = "S-1-16-8192";
    const uint TOKEN_ALL_ACCESS = 0xF01FF;
    const uint DISABLE_MAX_PRIVILEGE = 1, LUA_TOKEN = 4;
    const uint SE_GROUP_USE_FOR_DENY_ONLY = 0x10, SE_GROUP_INTEGRITY = 0x20;
    const int TokenGroups = 2, TokenIntegrityLevel = 25;
    const uint CREATE_UNICODE_ENVIRONMENT = 0x400, CREATE_NO_WINDOW = 0x08000000;

    [StructLayout(LayoutKind.Sequential)] struct SID_AND_ATTRIBUTES { public IntPtr Sid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)] struct PROCESS_INFORMATION { public IntPtr Process, Thread; public uint ProcessId, ThreadId; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct STARTUPINFO {
        public int Size; public string Reserved, Desktop, Title;
        public uint X, Y, Width, Height, CountChars, CountLines, Fill, Flags;
        public ushort Show, Reserved2; public IntPtr Reserved3, Input, Output, Error;
    }

    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool ConvertStringSidToSidW(string text, out IntPtr sid);
    [DllImport("advapi32.dll")] static extern int GetLengthSid(IntPtr sid);
    [DllImport("advapi32.dll")] static extern bool EqualSid(IntPtr a, IntPtr b);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool CreateRestrictedToken(IntPtr existing, uint flags, uint disableCount, SID_AND_ATTRIBUTES[] disable, uint deleteCount, IntPtr delete, uint restrictCount, IntPtr restrict, out IntPtr result);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool SetTokenInformation(IntPtr token, int kind, ref SID_AND_ATTRIBUTES label, int length);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool GetTokenInformation(IntPtr token, int kind, IntPtr buffer, int length, out int needed);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool CreateProcessAsUserW(IntPtr token, string application, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inherit, uint flags, string environment, string directory, ref STARTUPINFO startup, out PROCESS_INFORMATION information);

    static void Check(bool ok, string what) {
        if (!ok) throw new InvalidOperationException(what + " failed: Windows error " + Marshal.GetLastWin32Error());
    }
    static IntPtr Sid(string text) {
        IntPtr sid;
        Check(ConvertStringSidToSidW(text, out sid), "ConvertStringSidToSid");
        return sid;
    }

    // True only when the token holds Administrators and may use it for nothing but deny entries.
    static bool AdministratorsIsDenyOnly(IntPtr token) {
        int needed;
        GetTokenInformation(token, TokenGroups, IntPtr.Zero, 0, out needed);
        IntPtr buffer = Marshal.AllocHGlobal(needed);
        try {
            Check(GetTokenInformation(token, TokenGroups, buffer, needed, out needed), "GetTokenInformation");
            int count = Marshal.ReadInt32(buffer);
            int size = Marshal.SizeOf(typeof(SID_AND_ATTRIBUTES));
            IntPtr administrators = Sid(Administrators);
            for (int i = 0; i < count; i++) {
                var group = (SID_AND_ATTRIBUTES)Marshal.PtrToStructure(buffer + IntPtr.Size + i * size, typeof(SID_AND_ATTRIBUTES));
                if (EqualSid(group.Sid, administrators)) return (group.Attributes & SE_GROUP_USE_FOR_DENY_ONLY) != 0;
            }
            return false;
        } finally { Marshal.FreeHGlobal(buffer); }
    }

    // A restricted copy of the caller's own token needs no SeAssignPrimaryTokenPrivilege.
    public static int Start(string application, string directory, IDictionary environment) {
        IntPtr own, filtered;
        Check(OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, out own), "OpenProcessToken");
        try {
            var disable = new[] { new SID_AND_ATTRIBUTES { Sid = Sid(Administrators) } };
            Check(CreateRestrictedToken(own, DISABLE_MAX_PRIVILEGE | LUA_TOKEN, 1, disable, 0, IntPtr.Zero, 0, IntPtr.Zero, out filtered), "CreateRestrictedToken");
        } finally { CloseHandle(own); }
        try {
            var label = new SID_AND_ATTRIBUTES { Sid = Sid(MediumIntegrity), Attributes = SE_GROUP_INTEGRITY };
            Check(SetTokenInformation(filtered, TokenIntegrityLevel, ref label, Marshal.SizeOf(typeof(SID_AND_ATTRIBUTES)) + GetLengthSid(label.Sid)), "SetTokenInformation");
            if (!AdministratorsIsDenyOnly(filtered)) throw new InvalidOperationException("Administrators is not deny-only in the filtered token.");
            var names = new List<string>();
            foreach (DictionaryEntry entry in environment) names.Add(entry.Key + "=" + entry.Value);
            names.Sort(StringComparer.OrdinalIgnoreCase);
            var startup = new STARTUPINFO { Size = Marshal.SizeOf(typeof(STARTUPINFO)) };
            PROCESS_INFORMATION information;
            Check(CreateProcessAsUserW(filtered, application, new StringBuilder("\"" + application + "\""), IntPtr.Zero, IntPtr.Zero, false,
                CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW, string.Join("\0", names) + "\0", directory, ref startup, out information), "CreateProcessAsUser");
            CloseHandle(information.Thread);
            CloseHandle(information.Process);
            return (int)information.ProcessId;
        } finally { CloseHandle(filtered); }
    }
}
