# Real controller/server IPC with simulated native operations; never produces capture evidence.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'OcrPipe.ps1')
$tokens=$null; $errors=$null
$captureAst=[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Invoke-Capture.ps1'),[ref]$tokens,[ref]$errors)
foreach ($name in @('Read-Window','Read-Screenshot')) {
    $node=$captureAst.Find({param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true)
    Invoke-Expression $node.Extent.Text
}
$manifest=@{ocrReads=@();captures=@()}
function Start-TestHost {
    $start=[Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
    $start.UseShellExecute=$false; $start.CreateNoWindow=$true
    $start.RedirectStandardInput=$true; $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true
    $start.StandardInputEncoding=[Text.UTF8Encoding]::new($false)
    $start.StandardOutputEncoding=[Text.UTF8Encoding]::new($false)
    foreach ($arg in @('-NoLogo','-NoProfile','-File',(Join-Path $PSScriptRoot 'tests/OcrProtocolHost.ps1'))) { $start.ArgumentList.Add($arg) }
    [Diagnostics.Process]::Start($start)
}
function Stop-TestHost($Process) {
    if (-not $Process.HasExited) { $Process.Kill($true) }
    if (-not $Process.WaitForExit(5000)) { throw 'Test OCR host survived cleanup.' }
    $Process.Dispose()
}
$process=Start-TestHost
$ocr=$process
try {
    $first=Read-Window 'cold warm-up' 'warm-up' 10000
    $next=Read-Window 'Unicode path é, space and "quote"' 'test-read' 2000
    if ($first.pid -ne $next.pid -or $next.activations -ne 1) { throw 'OCR engine/process was not reused.' }
    if ($next.text -cne 'Unicode path é, space and "quote"') { throw 'JSON-lines UTF-8 path round trip failed.' }
    if ($next.words[0].x -ne 40 -or $next.words[0].height -ne 20) { throw 'OCR word boxes were lost.' }
    if ($manifest.ocrReads.Count -ne 2 -or -not $manifest.ocrReads[1].success -or $manifest.ocrReads[1].durationMs -lt 0) { throw 'OCR timings were not recorded.' }
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) { throw 'OCR host did not terminate cleanly on stdin EOF.' }
} finally { Stop-TestHost $process }
foreach ($case in @(
    @('error','simulated OCR unavailable'),@('eof','without a response'),@('invalid','JSON'),
    @('wrong-id','wrong request identity'),@('missing-result','no text/word result'),@('hang','exceeded 50 ms')
)) {
    $process=Start-TestHost
    $ocr=$process
    try {
        $null=Invoke-OcrRead $process 'warm-up' 1 10000
        $rejected=$false
        try {
            if ($case[0] -eq 'hang') { $null=Read-Window 'hang' 'test-timeout' 50 }
            else { $null=Invoke-OcrRead $process $case[0] 2 2000 }
        } catch {
            if ($_.Exception.Message -notmatch $case[1]) { throw "Wrong '$($case[0])' error: $($_.Exception.Message)" }
            $rejected=$true
        }
        if (-not $rejected) { throw "OCR protocol accepted '$($case[0])'." }
        if ($case[0] -eq 'hang' -and ($manifest.ocrReads[-1].success -or $manifest.ocrReads[-1].durationMs -lt 0)) { throw 'Failed OCR duration was not recorded.' }
    } finally { Stop-TestHost $process }
}
Write-Output 'OCR IPC passed: cold warm-up, one engine/process, UTF-8, word boxes, EOF, errors, invalid responses and bounded hang cleanup.'
# Exercise the actual screenshot coordinator with a file sentinel and failing OCR. This
# tests ordering/preservation only; the temporary sentinel is not an image or native evidence.
$OutDir=Join-Path ([IO.Path]::GetTempPath()) "dashboard-ocr-order-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $OutDir | Out-Null
try {
    $app=[pscustomobject]@{MainWindowTitle='Project Dashboard';HasExited=$false}
    function Focus-Window {}
    function Save-Window([string]$Path) { [IO.File]::WriteAllText($Path,'unit file sentinel'); @{rectangle=@{x=0;y=0;width=800;height=600};pixelVariance=10} }
    function Read-Window([string]$Path,[string]$State) { throw 'test OCR failure after saving' }
    $failed=$false
    try { $null=Read-Screenshot 'row-location' } catch {
        if ($_.Exception.Message -ne 'test OCR failure after saving') { throw }
        $failed=$true
    }
    if (-not $failed -or -not (Test-Path (Join-Path $OutDir 'row-location.png')) -or $manifest.captures.Count -ne 1) { throw 'First OCR failure lost its saved file or manifest entry.' }
    Write-Output 'First-read failure preserves the saved file and capture manifest entry.'
} finally { Remove-Item $OutDir -Recurse -Force }
