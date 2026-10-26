# One issue-31 attempt on the existing hosted Windows desktop. No product test switches.
param(
    [string]$Exe = 'target/release/project-dashboard.exe',
    [string]$OutDir = 'native-captures',
    [Parameter(Mandatory)][string]$ExpectedHead
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$root = Join-Path ([IO.Path]::GetTempPath()) "dashboard-native-$([Guid]::NewGuid().ToString('N'))"
$app = $null; $fixture = $null; $ocr = $null; $ocrErrors = $null
. (Join-Path $PSScriptRoot 'OcrPipe.ps1')
$manifest = [ordered]@{
    head = (git rev-parse HEAD); expectedHead = $ExpectedHead
    runner = 'GitHub-hosted windows-latest'; run = "https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
    syntheticLoopbackFixture = $true; success = $false; captures = @(); inputs = @(); ocrReads = @()
    pmdContractPin = '7cc05ab8c4c1c4015137c5dd047a4c5c39d60616'
    startedAt = [DateTimeOffset]::UtcNow.ToString('o')
    priorDesktopEvidence = @{run=40242202275;artifact=12617261372;file='window.png';sha256='b51b69dcd0fc8a8fb34803e7f734aa1937f5b8afa5078127200ee75ddb43a84c'}
}

# Child processes receive OS essentials only, never Actions/provider credentials.
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
    $evidence = Save-Window $path
    $capture = @{
        state=$State;file="$State.png";at=[DateTimeOffset]::UtcNow.ToString('o');sha256=(Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
        title=$app.MainWindowTitle;processAlive=(-not $app.HasExited);image=$evidence;contentVerified=$false;ocrReadId=$manifest.ocrReads.Count+1
    }
    $manifest.captures += $capture
    # Includes diagnostic screenshots: even the first row-location OCR failure leaves its PNG.
    $text = Read-Window $path $State
    $capture.visibleText = $text.text
    @{capture=$capture;text=$text}
}
function Click-Repository($Screenshot,[string]$Action) {
    Focus-Window
    $bounds = Window-Rect
    $saved = $Screenshot.capture.image.rectangle
    if ($bounds.X -ne $saved.x -or $bounds.Y -ne $saved.y -or $bounds.Width -ne $saved.width -or $bounds.Height -ne $saved.height) {
        throw 'Dashboard moved since OCR; refusing stale click coordinates.'
    }
    $words = @($Screenshot.text.words | Where-Object { $_.text -match 'fixture.?[o0]rg' })
    if ($words.Count -ne 1) { throw "Cannot locate one repository word for real mouse input. OCR: $($Screenshot.text.text)" }
    $word=$words[0]
    if ($word.width -le 0 -or $word.height -le 0) { throw 'OCR returned an invalid repository word box.' }
    $x=$bounds.X+[int]($word.x+$word.width/2); $y=$bounds.Y+[int]($word.y+$word.height/2)
    if (-not $bounds.Contains($x,$y) -or -not [NativeDesktop]::OwnsPoint($app.MainWindowHandle,$x,$y)) { throw 'Repository click target is not inside the owned Dashboard window.' }
    [NativeDesktop]::Click($x,$y)
    $manifest.inputs += @{kind='SendInput repository click';action=$Action;x=$x;y=$y;word=$word;at=[DateTimeOffset]::UtcNow.ToString('o')}
    Start-Sleep -Milliseconds 750
}
function Wait-Requests([string]$State) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if ($fixture.HasExited) { throw 'Loopback fixture exited.' }
        # Written only after the last work page, or an explicit no_status response.
        if (Test-Path (Join-Path $root "completed-$State")) { Start-Sleep -Seconds 2; return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "No completed production fetch for '$State' after OS input. Input delivery/refresh failed."
}
function Refresh-State([string]$State) {
    [IO.File]::WriteAllText((Join-Path $root 'next-state.txt'),$State)
    [IO.File]::Move((Join-Path $root 'next-state.txt'),(Join-Path $root 'state.txt'),$true)
    Focus-Window
    [NativeDesktop]::F5()
    $manifest.inputs += @{kind='SendInput F5';state=$State;at=[DateTimeOffset]::UtcNow.ToString('o')}
    Wait-Requests $State
}
function Capture-State([string]$State,[string[]]$Patterns) {
    $screenshot = Read-Screenshot $State
    $capture = $screenshot.capture; $text = $screenshot.text
    $visible = [regex]::Replace($text.text,'\s+',' ')
    # Missing markers fail this attempt; a nonblank window alone is insufficient evidence.
    foreach ($pattern in (@('Project Manager Board','truncated, complete list below','Unavailable: local health is not supplied by pmd','pmd status generated \S+ ago')+$Patterns)) {
        if ($visible -notmatch $pattern) { throw "Native '$State' image missing visible '$pattern'. OCR: $($text.text)" }
    }
    $capture.contentVerified = $true
    Write-Host "Native '$State' verified OCR: $($text.text)"
}
try {
    if (-not $IsWindows) { throw 'Native capture requires Windows.' }
    if ($manifest.head -ne $ExpectedHead) { throw 'Checkout is not the PR head.' }
    $Exe = (Resolve-Path $Exe).Path
    $manifest.exeSha256 = (Get-FileHash $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
    Add-Type -Path (Join-Path $PSScriptRoot 'NativeDesktop.cs')
    Add-Type -AssemblyName System.Windows.Forms,System.Drawing
    [void][NativeDesktop]::SetThreadDpiAwarenessContext([IntPtr](-4))
    $manifest.inputDesktop = [NativeDesktop]::InputDesktop()
    if (-not [Environment]::UserInteractive -or $manifest.inputDesktop -ne 'Default') { throw 'Locked/noninteractive runner desktop.' }
    $area = [Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    New-Item -ItemType Directory -Path (Join-Path $root 'checkout/.git') -Force | Out-Null
    # This fresh config has no saved UI state: product default zoom is 100%. Coordinates
    # come from physical screenshot word boxes, without changing display or zoom settings.
    Start-Ocr
    [IO.File]::WriteAllText((Join-Path $root 'checkout/.git/config'),"[remote `"origin`"]`nurl = https://github.com/fixture-org/synthetic-dashboard.git`n")
    [IO.File]::WriteAllText((Join-Path $root 'read-token'),'synthetic-issue31-token')
    [IO.File]::WriteAllText((Join-Path $root 'state.txt'),'normal')
    $fixture = Start-Owned (Join-Path $PSHOME 'pwsh.exe') @('-NoProfile','-File',(Join-Path $PSScriptRoot 'Start-LoopbackPmd.ps1'),'-Root',$root)
    $manifest.fixtureProcessId = $fixture.Id
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path (Join-Path $root 'port.txt'))) {
        if ($fixture.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'Synthetic loopback fixture did not start within 15 seconds.' }
        Start-Sleep -Milliseconds 100
    }
    $port = (Get-Content (Join-Path $root 'port.txt') -Raw).Trim()
    $manifest.fixtureUrl = "http://127.0.0.1:$port"
    $checkout = Join-Path $root 'checkout'
    [IO.File]::WriteAllText((Join-Path $root 'config.toml'),@"
repositories = ['$checkout']
refresh_minutes = 0
parallelism = 1
pmd_url = 'http://127.0.0.1:$port'
pmd_token_path = 'read-token'
t3_home = '$root'
"@)
    $app = Start-Owned $Exe @() @{PROJECT_DASHBOARD_CONFIG_DIR=$root}
    $manifest.dashboardProcessId = $app.Id
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 200; $app.Refresh()
        if ($app.HasExited) { throw "Dashboard exited before showing a window: $($app.ExitCode)" }
    } until ($app.MainWindowTitle -eq 'Project Dashboard' -or [DateTime]::UtcNow -ge $deadline)
    if ($app.MainWindowTitle -ne 'Project Dashboard') { throw 'No Dashboard window within 60 seconds.' }
    [void][NativeDesktop]::ShowWindow($app.MainWindowHandle,9)
    if (-not [NativeDesktop]::MoveWindow($app.MainWindowHandle,$area.X+8,$area.Y+8,[Math]::Min(1400,$area.Width-16),[Math]::Min(1000,$area.Height-16),$true)) { throw 'MoveWindow failed.' }
    Focus-Window
    Wait-Requests 'normal'
    $locator = Read-Screenshot 'row-location'
    Click-Repository $locator 'expand'
    $locator.capture.contentVerified = $true
    Start-Sleep -Seconds 2
    # The Board and work list are above local health and the Open issues table. Assert these
    # visible rows at the runner's existing size; an error can push Open issues below the fold.
    $normalRows = @('Ready.*?SYNTHETIC FIXTURE NORMAL.*?In progress','Open work.*?all API pages.*?SYNTHETIC FIXTURE NORMAL')
    Capture-State 'normal' $normalRows
    Refresh-State 'stale'
    # The small Cached pill is not reliably read by Windows OCR. The error, original-age
    # line and retained NORMAL Board/work rows together prove the last-good stale state.
    Capture-State 'stale' ($normalRows + @('pmd last refresh failed at','SYNTHETIC FIXTURE STALE upstream refresh failed'))
    Refresh-State 'unavailable'
    Capture-State 'unavailable' ($normalRows + @('pmd has no.?status for this project yet'))
    Refresh-State 'refreshed'
    Capture-State 'refresh' @('Ready.*?SYNTHETIC FIXTURE REFRESHED.*?In progress','Open work.*?all API pages.*?SYNTHETIC FIXTURE REFRESHED')
    if ($manifest.captures[-1].sha256 -eq $manifest.captures[-2].sha256) { throw 'OS refresh did not visibly change the image.' }
    Refresh-State 'board'
    Click-Repository $locator 'collapse-board'
    $collapsed = Read-Screenshot 'board-collapsed'
    if ($collapsed.text.text -match 'Project Manager Board') { throw 'Repository mouse input did not collapse Board details.' }
    Click-Repository $collapsed 'reopen-board'
    $collapsed.capture.contentVerified = $true
    Start-Sleep -Seconds 2
    Capture-State 'board' @('Ready.*?SYNTHETIC FIXTURE BOARD.*?In progress','Open work.*?all API pages.*?SYNTHETIC FIXTURE BOARD','Waiting on Franz','Blocked')
    $manifest.requests = @(Get-Content (Join-Path $root 'requests.jsonl') | ForEach-Object { $_ | ConvertFrom-Json })
    $manifest.success = $true
} catch {
    $manifest.failure = $_.Exception.Message
    throw
} finally {
    # Each owned child is stopped even if another cleanup operation fails.
    $cleanupErrors = @()
    foreach ($owned in @($app,$fixture,$ocr)) {
        try { Stop-Owned $owned } catch { $cleanupErrors += $_.Exception.Message }
    }
    if ($null -ne $ocrErrors -and $ocrErrors.IsCompleted) {
        try { $manifest.ocrHostDiagnostics = $ocrErrors.GetAwaiter().GetResult() } catch { $cleanupErrors += $_.Exception.Message }
    }
    try { if (Test-Path $root) { Remove-Item $root -Recurse -Force } } catch { $cleanupErrors += $_.Exception.Message }
    if ($cleanupErrors.Count) { $manifest.success=$false; $manifest.cleanupErrors=$cleanupErrors }
    $manifest.finishedAt = [DateTimeOffset]::UtcNow.ToString('o')
    $manifest | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $OutDir 'manifest.json')
    if ($cleanupErrors.Count) { throw "Native attempt cleanup failed: $($cleanupErrors -join '; ')" }
}
