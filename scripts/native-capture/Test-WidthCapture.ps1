# Geometry, text and PNG-before-OCR failure checks only. No native evidence or OS calls.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Invoke-WidthCapture.ps1'),[ref]$tokens,[ref]$errors)
if ($errors) { throw ($errors | Out-String) }
foreach ($name in @('Assert-ClientWidth','Assert-WidthText','Read-Screenshot','Capture-Width','Get-ModeRecord','Get-DisplayCode','Set-CaptureDisplay')) {
    $node=$ast.Find({param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true)
    if ($null -eq $node) { throw "Missing capture function $name" }
    Invoke-Expression $node.Extent.Text
}
function Reject([scriptblock]$Operation,[string]$Expected) {
    $failed=$false
    try { & $Operation } catch {
        if ($_.Exception.Message -notmatch $Expected) { throw }
        $failed=$true
    }
    if (-not $failed) { throw "Incorrectly accepted failure case: $Expected" }
}
Assert-ClientWidth 760 760
Reject { Assert-ClientWidth 1008 1680 } 'Client width mismatch'
$text='Project Dashboard Priority Updated fixture/long fixture/short'
Assert-WidthText $text 'baseline' 1680
Assert-WidthText $text 'head' 760
Assert-WidthText "$text DESCRIPTION SYNTHETIC SHORT" 'head' 1680
Reject { Assert-WidthText $text 'head' 1680 } 'DESCRIPTION'
Reject { Assert-WidthText "$text DESCRIPTION" 'head' 1680 } 'SYNTHETIC SHORT'
Reject { Assert-WidthText ($text.Replace('fixture/short','')) 'baseline' 760 } 'missing'
Reject { Assert-WidthText ($text.Replace('Priority','')) 'head' 760 } 'Priority'
Write-Output 'Width/text checks reject substituted client widths and missing row/header/description markers.'

# Compile the real interop declaration on mini and verify its Windows DEVMODEW ABI.
$code=(Get-Content (Join-Path $PSScriptRoot 'WidthDesktop.cs') -Raw).Replace('class WidthDesktop','class WidthDesktopAbi')
Add-Type -TypeDefinition $code
if ([Runtime.InteropServices.Marshal]::SizeOf([type][WidthDesktopAbi+Mode]) -ne 220 -or
    [Runtime.InteropServices.Marshal]::OffsetOf([type][WidthDesktopAbi+Mode],'Width').ToInt32() -ne 172) { throw 'DEVMODEW ABI mismatch.' }
function New-TestMode([int]$Index,[uint32]$Width,[uint32]$Height,[uint32]$Bpp) {
    $entry=[WidthDesktopAbi+EnumeratedMode]::new()
    $entry.Index=$Index
    $mode=[WidthDesktopAbi+Mode]::new()
    $mode.Width=$Width; $mode.Height=$Height; $mode.BitsPerPixel=$Bpp
    $mode.Frequency=75; $mode.Fields=12345; $mode.Flags=17; $mode.X=5; $mode.Orientation=2
    $entry.Settings=$mode
    $entry
}
$modes=[WidthDesktopAbi+EnumeratedMode[]]@(
    (New-TestMode 0 1024 768 32), (New-TestMode 1 1600 1200 16),
    (New-TestMode 2 1600 900 32), (New-TestMode 3 1280 1440 32),
    (New-TestMode 4 1600 1000 32)
)
$selected=[WidthDesktopAbi]::SelectWidest($modes)
if ($selected.Index -ne 4 -or -not $selected.Settings.Equals($modes[4].Settings)) { throw 'Mode selection lost width/bpp/height preference or changed the full DEVMODE.' }
$wider=New-TestMode 5 1800 900 16
if ([WidthDesktopAbi]::SelectWidest([WidthDesktopAbi+EnumeratedMode[]]@($modes[4],$wider)).Index -ne 5) { throw 'Mode selection did not prioritize width.' }
Reject { $null=[WidthDesktopAbi]::SelectWidest([WidthDesktopAbi+EnumeratedMode[]]@()) } 'no display modes'
Write-Output 'Real mode selector chooses an available non-1920 width, prefers 32 bpp then height, and preserves all DEVMODE fields.'

# Stub the native APIs to test coordinator ordering and failure preservation, never a real image.
Add-Type -TypeDefinition @'
using System;
public static class NativeDesktop {
    public struct RECT { public int Left,Top,Right,Bottom; }
    public static bool GetWindowRect(IntPtr window,out RECT rect) { rect=new RECT{Right=776,Bottom=636}; return true; }
}
public static class WidthDesktop {
    public struct Mode { public uint Width,Height,BitsPerPixel,Frequency,Fields; }
    public struct EnumeratedMode { public int Index; public Mode Settings; }
    public static EnumeratedMode[] Modes = new [] {
        new EnumeratedMode { Index=0, Settings=new Mode { Width=1024,Height=768,BitsPerPixel=32,Frequency=60,Fields=12345 } },
        new EnumeratedMode { Index=1, Settings=new Mode { Width=1600,Height=900,BitsPerPixel=16,Frequency=60,Fields=12345 } },
        new EnumeratedMode { Index=2, Settings=new Mode { Width=1600,Height=900,BitsPerPixel=32,Frequency=75,Fields=12345 } }
    };
    public static int TestCode=0, ApplyCalls=0;
    public static Mode ReadMode(int index) { return Modes[ApplyCalls>0 ? 2 : 0].Settings; }
    public static EnumeratedMode[] EnumerateModes() { return Modes; }
    public static EnumeratedMode SelectWidest(EnumeratedMode[] modes) { return modes[2]; }
    public static int ChangeDisplaySettingsExW(string device,ref Mode mode,IntPtr window,uint flags,IntPtr parameter) {
        if (mode.Fields!=12345 || mode.Width!=1600 || mode.Frequency!=75) throw new Exception("Changed enumerated DEVMODE");
        if (flags==2) { mode.Fields=99; return TestCode; }
        if (flags!=0) throw new Exception("Unexpected display flags");
        ApplyCalls++; return 0;
    }
    public struct Rect { public int Left,Top,Right,Bottom; }
    public static Rect WorkArea() { var mode=ReadMode(-1); return new Rect { Right=(int)mode.Width,Bottom=(int)mode.Height-40 }; }
    public static int Width=760;
    public static Rect Client(IntPtr window) { return new Rect{Right=Width,Bottom=620}; }
    public static uint GetDpiForWindow(IntPtr window) { return 96; }
    public static void ResizeClient(IntPtr window,int x,int y,int width,int height) { Width=width; }
}
'@
$manifest=@{}
Set-CaptureDisplay
if ($manifest.display.enumeratedModes.Count -ne 3 -or $manifest.display.enumeratedModes[1].bitsPerPixel -ne 16 -or
    $manifest.display.enumeratedModes[2].index -ne 2 -or $manifest.display.enumeratedModes[2].frequency -ne 75 -or
    $manifest.display.requested.width -ne 1600 -or $manifest.display.actual.width -ne 1600 -or
    -not $displayApplied -or [WidthDesktop]::ApplyCalls -ne 1 -or $manifest.display.registryUpdated) { throw 'Mode enumeration, untouched DEVMODE application or actual resolution evidence was lost.' }
[WidthDesktop]::ApplyCalls=0; [WidthDesktop]::TestCode=-5
$displayApplied=$false
Set-CaptureDisplay
if ($manifest.display.enumeratedModes.Count -ne 3 -or $manifest.display.testCode -ne -5 -or
    $manifest.display.actual.width -ne 1024 -or $displayApplied -or [WidthDesktop]::ApplyCalls -ne 0 -or
    $manifest.display.errors[0] -notmatch 'DISP_CHANGE_BADPARAM') { throw 'Rejected mode lost enumeration/API error evidence or was applied.' }
Write-Output 'Display controller records every mode, applies only an untouched tested mode, and retains the verified desktop and API error on rejection.'
$OutDir=Join-Path ([IO.Path]::GetTempPath()) "width-order-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $OutDir | Out-Null
try {
    $manifest=@{captures=@();ocrReads=@();skipped=@();display=@{errors=@('DISP_CHANGE_BADMODE')}}
    $app=[pscustomobject]@{MainWindowTitle='Project Dashboard';HasExited=$false;MainWindowHandle=[IntPtr]1;Id=42}
    $activeSource=@{source='baseline';commit='pinned-baseline';exeSha256='fixture-hash'}
    $widthProfile=@{name='default';zoom=1.0;repositoryRem=18.0}
    $currentWidth=760
    function Focus-Window {}
    function Save-Window([string]$Path) { [IO.File]::WriteAllText($Path,'unit file sentinel'); @{rectangle=@{x=0;y=0;width=776;height=636};pixelVariance=10} }
    function Read-Window([string]$Path,[string]$State) { throw 'simulated OCR failure after saving' }
    Reject { $null=Read-Screenshot 'baseline-default-760' } 'simulated OCR failure after saving'
    $saved=$manifest.captures[0]
    if (-not (Test-Path (Join-Path $OutDir 'baseline-default-760.png')) -or $saved.contentVerified -or
        $saved.commit -ne 'pinned-baseline' -or $saved.actualClientWidth -ne 760 -or $saved.zoom -ne 1.0 -or
        $saved.sha256 -ne (Get-FileHash (Join-Path $OutDir 'baseline-default-760.png')).Hash.ToLowerInvariant()) { throw 'Lost failed-read PNG provenance.' }
    [WidthDesktop]::Width=1008
    Reject { $null=Read-Screenshot 'wrong-width' } 'Client width mismatch'
    if (Test-Path (Join-Path $OutDir 'wrong-width.png')) { throw 'Wrong client width produced accepted evidence.' }
    [WidthDesktop]::Width=760
    $area=[pscustomobject]@{Width=1024;Height=728}
    Capture-Width 'too-wide' 1680
    if ($manifest.skipped.Count -ne 1 -or $manifest.skipped[0].reason -notmatch 'DISP_CHANGE_BADMODE' -or
        $manifest.skipped[0].requestedClientWidth -ne 1680 -or (Test-Path (Join-Path $OutDir 'too-wide.png'))) { throw 'Unsupported width was hidden or substituted.' }
    Write-Output 'Failed OCR preserves PNG/hash/source/width/zoom; unsupported desktop widths retain the API error without a substitute image.'
} finally { Remove-Item $OutDir -Recurse -Force }
