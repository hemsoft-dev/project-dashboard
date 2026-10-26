# Issue-27 width comparison only. The process, PNG and warm OCR helpers follow Invoke-Capture.ps1.
param(
    [Parameter(Mandatory)][string]$ExpectedHead,
    [string]$BinaryDir = 'native-width-binaries',
    [string]$OutDir = 'native-width-captures'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$root = Join-Path ([IO.Path]::GetTempPath()) "dashboard-width-$([Guid]::NewGuid().ToString('N'))"
$app=$null; $fixture=$null; $ocr=$null; $ocrErrors=$null; $originalMode=$null; $displayApplied=$false
. (Join-Path $PSScriptRoot 'OcrPipe.ps1')
$baseline = '4cc273ac30fe091fb9e898c85d004831648cb8c8'
$manifest = [ordered]@{
    head=(git rev-parse HEAD); expectedHead=$ExpectedHead; baseline=$baseline
    runner='GitHub-hosted windows-latest'; run="https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
    syntheticLoopbackFixture=$true; success=$false; ownerInspectionPending=$true
    captures=@(); ocrReads=@(); skipped=@(); processes=@()
    profiles=@(@{name='default';zoom=1.0;repositoryRem=18.0},@{name='saved';zoom=1.3;repositoryRem=20.88})
    requestedClientWidths=@(760,1100,1680); requestedClientHeight=620
    startedAt=[DateTimeOffset]::UtcNow.ToString('o')
}

function New-OwnedStart([string]$File, [string[]]$Arguments, [hashtable]$Extra = @{}) {
    $start = [Diagnostics.ProcessStartInfo]::new($File)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.Environment.Clear()
    foreach ($name in @('PATH','SystemRoot','WINDIR','USERPROFILE','APPDATA','LOCALAPPDATA','TEMP','TMP','COMSPEC','PATHEXT','ProgramFiles','ProgramFiles(x86)','ProgramData','SystemDrive')) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value) { $start.Environment[$name] = $value }
    }
    foreach ($name in $Extra.Keys) { $start.Environment[$name] = $Extra[$name] }
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $start
}

function Start-Owned([string]$File, [string[]]$Arguments, [hashtable]$Extra = @{}) {
    [Diagnostics.Process]::Start((New-OwnedStart $File $Arguments $Extra))
}

function Stop-Owned($Process) {
    if ($null -ne $Process) {
        if (-not $Process.HasExited) { $Process.Kill($true); [void]$Process.WaitForExit(5000) }
        if (-not $Process.HasExited) { throw "Owned process $($Process.Id) did not stop." }
        $Process.Dispose()
    }
}

function Window-Rect {
    $app.Refresh()
    if ($app.HasExited) { throw 'Dashboard exited during the native attempt.' }
    if ($app.MainWindowTitle -cne 'Project Dashboard') { throw 'Owned window title changed.' }
    [uint32]$ownerId=0
    if ([WidthDesktop]::GetWindowThreadProcessId($app.MainWindowHandle,[ref]$ownerId) -eq 0 -or $ownerId -ne $app.Id) { throw 'Native window process ownership mismatch.' }
    $expectedExe=Join-Path $BinaryDir "$($activeSource.source).exe"
    if ($app.MainModule.FileName -ine $expectedExe) { throw 'Native window executable path mismatch.' }
    if ([NativeDesktop]::InputDesktop() -ne 'Default') { throw 'Locked/noninteractive input desktop.' }
    if ([NativeDesktop]::GetForegroundWindow() -ne $app.MainWindowHandle) { throw 'Input/capture refused: Dashboard is not foreground.' }
    $rect = [NativeDesktop+RECT]::new()
    if (-not [NativeDesktop]::GetWindowRect($app.MainWindowHandle,[ref]$rect)) { throw 'GetWindowRect failed.' }
    $bounds = [Drawing.Rectangle]::FromLTRB($rect.Left,$rect.Top,$rect.Right,$rect.Bottom)
    if ($bounds.Width -lt 600 -or $bounds.Height -lt 500 -or -not $area.Contains($bounds)) {
        throw 'Owned window rectangle is too small or outside the visible working area.'
    }
    $bounds
}

function Focus-Window {
    [void][NativeDesktop]::SetForegroundWindow($app.MainWindowHandle)
    $deadline = [DateTime]::UtcNow.AddSeconds(2)
    while ([NativeDesktop]::GetForegroundWindow() -ne $app.MainWindowHandle -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 50
    }
    $null = Window-Rect
}

function Save-Window([string]$Path) {
    $bounds = Window-Rect
    $bitmap = [Drawing.Bitmap]::new($bounds.Width,$bounds.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        # Never photograph the desktop: only GetWindowRect of this owned foreground process.
        $graphics.CopyFromScreen($bounds.Location,[Drawing.Point]::Empty,$bounds.Size)
        $sum = 0.0; $sum2 = 0.0; $count = 0; $min = 255; $max = 0
        for ($y=0; $y -lt $bitmap.Height; $y+=4) {
            for ($x=0; $x -lt $bitmap.Width; $x+=4) {
                $pixel = $bitmap.GetPixel($x,$y)
                $v = ([int]$pixel.R+[int]$pixel.G+[int]$pixel.B)/3
                $sum += $v; $sum2 += $v*$v; $count++; $min=[Math]::Min($min,$v); $max=[Math]::Max($max,$v)
            }
        }
        $variance = $sum2/$count-[Math]::Pow($sum/$count,2)
        if ($variance -lt 2 -or $max-$min -lt 40) { throw 'Blank/uniform native image: interactive desktop unavailable.' }
        $bitmap.Save($Path,[Drawing.Imaging.ImageFormat]::Png)
        @{rectangle=@{x=$bounds.X;y=$bounds.Y;width=$bounds.Width;height=$bounds.Height};pixelVariance=$variance}
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}

function Read-Window([string]$Path,[string]$State,[int]$TimeoutMs=60000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $read = @{id=$manifest.ocrReads.Count+1;state=$State;file=[IO.Path]::GetFileName($Path);timeoutMs=$TimeoutMs;success=$false;at=[DateTimeOffset]::UtcNow.ToString('o')}
    $manifest.ocrReads += $read
    try {
        $response = Invoke-OcrRead $ocr $Path $read.id $TimeoutMs
        $read.hostDurationMs = $response.durationMs
        $read.success = $true
        $response.result
    } catch {
        $read.error = $_.Exception.Message
        if ($State -eq 'warm-up') { throw "Initialization/warm-up request failed: $($read.error)" }
        throw "Native OCR failed after warm-up for '$State': $($read.error)"
    } finally {
        $read.durationMs = $watch.ElapsedMilliseconds
        Write-Host "OCR '$State': $($read.durationMs) ms, success=$($read.success)"
    }
}

function Start-Ocr {
    # Cold WinRT/module activation happens once, outside the steady-state per-image budget.
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $manifest.ocrWarmUp = @{timeoutMs=120000;success=$false}
    try {
        $start = New-OwnedStart (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe') @('-NoLogo','-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'Read-WindowText.ps1'))
        $start.RedirectStandardInput=$true; $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true
        $start.StandardInputEncoding=[Text.UTF8Encoding]::new($false)
        $start.StandardOutputEncoding=[Text.UTF8Encoding]::new($false)
        $script:ocr = [Diagnostics.Process]::Start($start)
        $script:ocrErrors = $ocr.StandardError.ReadToEndAsync()
        $manifest.ocrProcessId = $ocr.Id
        $path = Join-Path $root 'ocr-warm-up.png'
        $bitmap = [Drawing.Bitmap]::new(512,128)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        $font = [Drawing.Font]::new([Drawing.FontFamily]::GenericSansSerif,32,[Drawing.FontStyle]::Bold)
        try {
            $graphics.Clear([Drawing.Color]::White)
            $graphics.DrawString('OCR READY',$font,[Drawing.Brushes]::Black,16,32)
            $bitmap.Save($path,[Drawing.Imaging.ImageFormat]::Png)
        } finally { $font.Dispose(); $graphics.Dispose(); $bitmap.Dispose() }
        $remaining = 120000-[int]$watch.ElapsedMilliseconds
        if ($remaining -le 0) { throw 'OCR initialization exhausted the warm-up budget before reading its image.' }
        $text = Read-Window $path 'warm-up' $remaining
        if ([string]::IsNullOrWhiteSpace($text.text)) { throw 'Warm-up image produced no recognized text.' }
        $manifest.ocrWarmUp.success = $true
    } catch {
        $manifest.ocrWarmUp.error = $_.Exception.Message
        throw "OCR unavailable on runner: initialization/warm-up failed within its 120-second bound: $($manifest.ocrWarmUp.error)"
    } finally {
        $manifest.ocrWarmUp.durationMs = $watch.ElapsedMilliseconds
        Write-Host "OCR initialization and warm-up: $($manifest.ocrWarmUp.durationMs) ms, success=$($manifest.ocrWarmUp.success)"
    }
}

function Read-Screenshot([string]$State) {
    Focus-Window
    $path = Join-Path $OutDir "$State.png"
    $client = [WidthDesktop]::Client($app.MainWindowHandle)
    Assert-ClientWidth ($client.Right-$client.Left) $currentWidth
    $evidence = Save-Window $path
    $capture = @{
        source=$activeSource.source;commit=$activeSource.commit;exeSha256=$activeSource.exeSha256
        requestedClientWidth=$currentWidth;actualClientWidth=($client.Right-$client.Left);actualClientHeight=($client.Bottom-$client.Top)
        zoom=$widthProfile.zoom;repositoryRem=$widthProfile.repositoryRem;profile=$widthProfile.name;dpi=[WidthDesktop]::GetDpiForWindow($app.MainWindowHandle)
        state=$State;file="$State.png";at=[DateTimeOffset]::UtcNow.ToString('o');sha256=(Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
        title=$app.MainWindowTitle;processId=$app.Id;processAlive=(-not $app.HasExited);windowOwnerVerified=$true
        image=$evidence;contentVerified=$false;ocrReadId=$manifest.ocrReads.Count+1
    }
    $manifest.captures += $capture
    # Even the first OCR failure leaves its PNG and source/geometry/hash entry.
    $text = Read-Window $path $State
    $capture.visibleText = $text.text
    @{capture=$capture;text=$text}
}

function Assert-ClientWidth([int]$Actual,[int]$Requested) {
    if ($Actual -ne $Requested) { throw "Client width mismatch: requested $Requested px, actual $Actual px. No scaled or clipped substitute is accepted." }
}
function Assert-WidthText([string]$Text,[string]$Source,[int]$Width) {
    $visible=[regex]::Replace($Text,'\s+',' ')
    # Semibold repository identities and large headers establish both rows in every image.
    # Small pills and truncated activity strings are deliberately not narrow/baseline assertions.
    $patterns=@('Project Dashboard','Priority','Updated','fixture\s*[/\\|]\s*long','fixture\s*[/\\|]\s*short')
    if ($Source -eq 'head' -and $Width -eq 1680) { $patterns += @('DESCRIPTION','SYNTHETIC SHORT') }
    foreach ($pattern in $patterns) {
        if ($visible -notmatch $pattern) { throw "Native width image missing '$pattern'. OCR: $Text" }
    }
}
function Get-ModeRecord($Mode) {
    @{width=$Mode.Width;height=$Mode.Height;bitsPerPixel=$Mode.BitsPerPixel;frequency=$Mode.Frequency}
}
function Get-DisplayCode([int]$Code) {
    $names=@{0='DISP_CHANGE_SUCCESSFUL';1='DISP_CHANGE_RESTART';'-1'='DISP_CHANGE_FAILED';'-2'='DISP_CHANGE_BADMODE';'-3'='DISP_CHANGE_NOTUPDATED';'-4'='DISP_CHANGE_BADFLAGS';'-5'='DISP_CHANGE_BADPARAM';'-6'='DISP_CHANGE_BADDUALVIEW'}
    # String keys also work consistently with negative PowerShell hashtable keys.
    foreach ($key in $names.Keys) { if ([int]$key -eq $Code) { return $names[$key] } }
    "UNKNOWN_DISPLAY_RESULT_$Code"
}
function Set-CaptureDisplay {
    $manifest.display=@{api='EnumDisplaySettingsW + ChangeDisplaySettingsExW';requested=@{width=1920;height=1080};registryUpdated=$false;errors=@()}
    try {
        $script:originalMode=[WidthDesktop]::ReadMode(-1)
        $manifest.display.before=Get-ModeRecord $originalMode
        if ($originalMode.Width -ne 1920 -or $originalMode.Height -ne 1080) {
            $desired=[WidthDesktop]::DesiredMode()
            $code=[WidthDesktop]::ChangeDisplaySettingsExW($null,[ref]$desired,[IntPtr]::Zero,2,[IntPtr]::Zero)
            $manifest.display.testCode=$code; $manifest.display.testResult=Get-DisplayCode $code
            if ($code -eq 0) {
                # Flags=0 changes the live mode only. Never CDS_UPDATEREGISTRY.
                $code=[WidthDesktop]::ChangeDisplaySettingsExW($null,[ref]$desired,[IntPtr]::Zero,0,[IntPtr]::Zero)
                $manifest.display.applyCode=$code; $manifest.display.applyResult=Get-DisplayCode $code
                $script:displayApplied=($code -eq 0)
                if ($code -ne 0) { $manifest.display.errors += "ChangeDisplaySettingsExW apply: $code ($(Get-DisplayCode $code))" }
                Start-Sleep -Seconds 2
            } else { $manifest.display.errors += "ChangeDisplaySettingsExW CDS_TEST: $code ($(Get-DisplayCode $code))" }
        }
    } catch { $manifest.display.errors += $_.Exception.Message }
    $actual=[WidthDesktop]::ReadMode(-1)
    $manifest.display.actual=Get-ModeRecord $actual
    $work=[WidthDesktop]::WorkArea()
    $script:area=[Drawing.Rectangle]::FromLTRB($work.Left,$work.Top,$work.Right,$work.Bottom)
    $manifest.display.workingArea=@{x=$area.X;y=$area.Y;width=$area.Width;height=$area.Height}
    Write-Host "Native display: $($actual.Width)x$($actual.Height); work area $area; API errors: $($manifest.display.errors -join '; ')"
}
function Wait-WidthRequests([string]$Session) {
    $required=@('/v1/projects','/v1/projects/long/status','/v1/projects/short/status','/v1/board?project=long','/v1/board?project=short','/v1/work?project=long&limit=500','/v1/work?project=short&limit=500')
    $deadline=[DateTime]::UtcNow.AddSeconds(20)
    do {
        if ($fixture.HasExited -or $app.HasExited) { throw 'Owned fixture or Dashboard exited during loading.' }
        $seen=@()
        if (Test-Path (Join-Path $root 'requests.jsonl')) {
            # A writer can have the last line open; wait for complete JSON lines on the next tick.
            try {
                $seen=@(Get-Content (Join-Path $root 'requests.jsonl') | ForEach-Object { $_ | ConvertFrom-Json } |
                    Where-Object { $_.session -eq $Session -and $_.status -eq 200 } | ForEach-Object { $_.path })
            } catch { $seen=@() }
        }
        if (@($required | Where-Object { $_ -notin $seen }).Count -eq 0) { Start-Sleep -Seconds 2; return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "No complete authenticated production fetch for session '$Session' within 20 seconds."
}
function Start-WidthApp([string]$Session) {
    $config=Join-Path $root $Session
    New-Item -ItemType Directory $config | Out-Null
    $long=Join-Path $root 'long'; $short=Join-Path $root 'short'
    [IO.File]::WriteAllText((Join-Path $config 'config.toml'),@"
repositories = ['$long', '$short']
refresh_minutes = 0
parallelism = 1
pmd_url = '$($manifest.fixtureUrl)'
pmd_token_path = '../read-token'
t3_home = '$config'
"@)
    @{zoom=$widthProfile.zoom;columns=@{repository=$widthProfile.repositoryRem}} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $config 'ui-state.json')
    [IO.File]::WriteAllText((Join-Path $root 'session.txt'),$Session)
    $script:app=Start-Owned (Join-Path $BinaryDir "$($activeSource.source).exe") @() @{PROJECT_DASHBOARD_CONFIG_DIR=$config}
    $manifest.processes += @{session=$Session;source=$activeSource.source;processId=$app.Id;zoom=$widthProfile.zoom;repositoryRem=$widthProfile.repositoryRem}
    $deadline=[DateTime]::UtcNow.AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 200; $app.Refresh()
        if ($app.HasExited) { throw "Dashboard exited before showing a window: $($app.ExitCode)" }
    } until ($app.MainWindowTitle -eq 'Project Dashboard' -or [DateTime]::UtcNow -ge $deadline)
    if ($app.MainWindowTitle -ne 'Project Dashboard') { throw 'No Dashboard window within 60 seconds.' }
    [void][NativeDesktop]::ShowWindow($app.MainWindowHandle,9)
    Wait-WidthRequests $Session
}
function Capture-Width([string]$State,[int]$Width) {
    $outer=[NativeDesktop+RECT]::new()
    if (-not [NativeDesktop]::GetWindowRect($app.MainWindowHandle,[ref]$outer)) { throw 'GetWindowRect before sizing failed.' }
    $client=[WidthDesktop]::Client($app.MainWindowHandle)
    $extraWidth=($outer.Right-$outer.Left)-($client.Right-$client.Left)
    $extraHeight=($outer.Bottom-$outer.Top)-($client.Bottom-$client.Top)
    if ($Width+$extraWidth -gt $area.Width-16 -or 620+$extraHeight -gt $area.Height-16) {
        $reason="Client ${Width}x620 plus frame ${extraWidth}x${extraHeight} does not fit work area $($area.Width)x$($area.Height). Display API errors: $($manifest.display.errors -join '; ')"
        $manifest.skipped += @{source=$activeSource.source;commit=$activeSource.commit;profile=$widthProfile.name;zoom=$widthProfile.zoom;requestedClientWidth=$Width;reason=$reason}
        Write-Warning $reason
        return
    }
    [WidthDesktop]::ResizeClient($app.MainWindowHandle,$area.X+8,$area.Y+8,$Width,620)
    Start-Sleep -Milliseconds 750
    $measured=[WidthDesktop]::Client($app.MainWindowHandle)
    Assert-ClientWidth ($measured.Right-$measured.Left) $Width
    if ($measured.Bottom-$measured.Top -ne 620) { throw 'Native client height is not the requested 620 px.' }
    $screenshot=Read-Screenshot $State
    Assert-WidthText $screenshot.text.text $activeSource.source $Width
    $screenshot.capture.contentVerified=$true
    Write-Host "Native '$State' client width $Width verified. OCR: $($screenshot.text.text)"
}
try {
    if (-not $IsWindows) { throw 'Native width capture requires Windows.' }
    if ($manifest.head -cne $ExpectedHead) { throw 'Checkout is not the PR head.' }
    $BinaryDir=(Resolve-Path $BinaryDir).Path
    $build=Get-Content (Join-Path $OutDir 'build-manifest.json') -Raw | ConvertFrom-Json
    if (-not $build.success -or $build.head -cne $ExpectedHead -or $build.baseline -cne $baseline -or $build.sources.Count -ne 2) { throw 'Binary build provenance mismatch.' }
    foreach ($source in $build.sources) {
        $expected=if ($source.source -eq 'head') { $ExpectedHead } elseif ($source.source -eq 'baseline') { $baseline } else { throw 'Unknown binary source.' }
        if ($source.commit -cne $expected -or (Get-FileHash (Join-Path $BinaryDir "$($source.source).exe")).Hash.ToLowerInvariant() -cne $source.exeSha256) { throw 'Executable hash/commit mismatch.' }
    }
    $manifest.sources=$build.sources
    Add-Type -Path (Join-Path $PSScriptRoot 'NativeDesktop.cs')
    Add-Type -Path (Join-Path $PSScriptRoot 'WidthDesktop.cs')
    Add-Type -AssemblyName System.Windows.Forms,System.Drawing
    [void][NativeDesktop]::SetThreadDpiAwarenessContext([IntPtr](-4))
    $manifest.inputDesktop=[NativeDesktop]::InputDesktop()
    if (-not [Environment]::UserInteractive -or $manifest.inputDesktop -ne 'Default') { throw 'Locked/noninteractive runner desktop.' }
    New-Item -ItemType Directory $root | Out-Null
    Set-CaptureDisplay
    Start-Ocr
    foreach ($kind in @('long','short')) {
        New-Item -ItemType Directory (Join-Path $root "$kind/.git") -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $root "$kind/.git/config"),"[remote `"origin`"]`nurl = https://github.com/fixture/$kind.git`n")
    }
    [IO.File]::WriteAllText((Join-Path $root 'read-token'),'synthetic-issue27-token')
    [IO.File]::WriteAllText((Join-Path $root 'session.txt'),'startup')
    $fixture=Start-Owned (Join-Path $PSHOME 'pwsh.exe') @('-NoProfile','-File',(Join-Path $PSScriptRoot 'Start-WidthPmd.ps1'),'-Root',$root)
    $manifest.fixtureProcessId=$fixture.Id
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path (Join-Path $root 'port.txt'))) {
        if ($fixture.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'Synthetic width fixture did not start within 15 seconds.' }
        Start-Sleep -Milliseconds 100
    }
    $port=(Get-Content (Join-Path $root 'port.txt') -Raw).Trim()
    $manifest.fixtureUrl="http://127.0.0.1:$port"
    Copy-Item (Join-Path $root 'fixture-data.json') $OutDir
    $manifest.fixtureSha256=(Get-FileHash (Join-Path $OutDir 'fixture-data.json')).Hash.ToLowerInvariant()
    foreach ($activeSource in @($build.sources | Sort-Object source)) {
        foreach ($widthProfile in $manifest.profiles) {
            $session="$($activeSource.source)-$($widthProfile.name)"
            try {
                Start-WidthApp $session
                foreach ($currentWidth in $manifest.requestedClientWidths) { Capture-Width "$session-$currentWidth" $currentWidth }
            } finally { Stop-Owned $app; $app=$null }
        }
    }
    if ($manifest.captures.Count -eq 0) { throw 'No requested width fits the runner desktop.' }
    $manifest.matrixComplete=($manifest.skipped.Count -eq 0 -and $manifest.captures.Count -eq 12)
    if (-not $manifest.matrixComplete) { Write-Warning 'Width matrix is incomplete. See skipped entries and display API results; owner acceptance remains pending.' }
    $manifest.success=$true
} catch {
    $manifest.failure=$_.Exception.Message
    throw
} finally {
    $cleanupErrors=@()
    foreach ($owned in @($app,$fixture,$ocr)) {
        try { Stop-Owned $owned } catch { $cleanupErrors += $_.Exception.Message }
    }
    if ($null -ne $ocrErrors -and $ocrErrors.IsCompleted) {
        try { $manifest.ocrHostDiagnostics=$ocrErrors.GetAwaiter().GetResult() } catch { $cleanupErrors += $_.Exception.Message }
    }
    try { if (Test-Path (Join-Path $root 'requests.jsonl')) { Copy-Item (Join-Path $root 'requests.jsonl') $OutDir } } catch { $cleanupErrors += $_.Exception.Message }
    if ($displayApplied -and $null -ne $originalMode) {
        try {
            $code=[WidthDesktop]::ChangeDisplaySettingsExW($null,[ref]$originalMode,[IntPtr]::Zero,0,[IntPtr]::Zero)
            $manifest.display.restoreCode=$code; $manifest.display.restoreResult=Get-DisplayCode $code
            $restored=[WidthDesktop]::ReadMode(-1)
            $manifest.display.restored=Get-ModeRecord $restored
            if ($code -ne 0 -or $restored.Width -ne $originalMode.Width -or $restored.Height -ne $originalMode.Height) { throw "Display restore failed: $code" }
        } catch { $cleanupErrors += $_.Exception.Message }
    }
    try { if (Test-Path $root) { Remove-Item $root -Recurse -Force } } catch { $cleanupErrors += $_.Exception.Message }
    if ($cleanupErrors.Count) { $manifest.success=$false; $manifest.cleanupErrors=$cleanupErrors }
    $manifest.finishedAt=[DateTimeOffset]::UtcNow.ToString('o')
    $manifest | ConvertTo-Json -Depth 14 | Set-Content (Join-Path $OutDir 'manifest.json')
    if ($cleanupErrors.Count) { throw "Width capture cleanup failed: $($cleanupErrors -join '; ')" }
}
