# Geometry, text and PNG-before-OCR failure checks only. No native evidence or OS calls.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Invoke-WidthCapture.ps1'),[ref]$tokens,[ref]$errors)
if ($errors) { throw ($errors | Out-String) }
foreach ($name in @('Assert-ClientWidth','Assert-WidthText','Read-Screenshot','Capture-Width')) {
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

# Stub the native APIs to test coordinator ordering and failure preservation, never a real image.
Add-Type -TypeDefinition @'
using System;
public static class NativeDesktop {
    public struct RECT { public int Left,Top,Right,Bottom; }
    public static bool GetWindowRect(IntPtr window,out RECT rect) { rect=new RECT{Right=776,Bottom=636}; return true; }
}
public static class WidthDesktop {
    public struct Rect { public int Left,Top,Right,Bottom; }
    public static int Width=760;
    public static Rect Client(IntPtr window) { return new Rect{Right=Width,Bottom=620}; }
    public static uint GetDpiForWindow(IntPtr window) { return 96; }
    public static void ResizeClient(IntPtr window,int x,int y,int width,int height) { Width=width; }
}
'@
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
