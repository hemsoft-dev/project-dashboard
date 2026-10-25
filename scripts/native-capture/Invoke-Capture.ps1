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
$app = $null; $fixture = $null
$manifest = [ordered]@{
    head = (git rev-parse HEAD); expectedHead = $ExpectedHead
    runner = 'GitHub-hosted windows-latest'; run = "https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
    syntheticLoopbackFixture = $true; success = $false; captures = @(); inputs = @()
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
function Read-Window([string]$Path) {
    # Built-in Windows OCR verifies visible content and locates the repository for a real mouse click.
    $start = New-OwnedStart (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe') @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'Read-WindowText.ps1'),'-Image',$Path)
    $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true
    $ocr = [Diagnostics.Process]::Start($start)
    try {
        $stdout=$ocr.StandardOutput.ReadToEndAsync(); $stderr=$ocr.StandardError.ReadToEndAsync()
        if (-not $ocr.WaitForExit(45000)) { throw 'Native OCR exceeded 45 seconds.' }
        if ($ocr.ExitCode -ne 0) { throw "Native OCR failed: $($stderr.Result)" }
        $stdout.Result | ConvertFrom-Json
    } finally { Stop-Owned $ocr }
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
    $null = Window-Rect
    [NativeDesktop]::F5()
    $manifest.inputs += @{kind='SendInput F5';state=$State;at=[DateTimeOffset]::UtcNow.ToString('o')}
    Wait-Requests $State
}
function Capture-State([string]$State,[string[]]$Patterns) {
    $file = "$State.png"; $path = Join-Path $OutDir $file
    $evidence = Save-Window $path
    $capture = @{
        state=$State;file=$file;at=[DateTimeOffset]::UtcNow.ToString('o');sha256=(Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
        title=$app.MainWindowTitle;processAlive=(-not $app.HasExited);image=$evidence;contentVerified=$false
    }
    $manifest.captures += $capture
    $text = Read-Window $path
    $capture.visibleText = $text.text
    $visible = [regex]::Replace($text.text,'\s+',' ')
    # Missing markers fail this attempt; a nonblank window alone is insufficient evidence.
    foreach ($pattern in (@('Project Manager Board','truncated','not supplied by pmd','pmd status generated')+$Patterns)) {
        if ($visible -notmatch $pattern) { throw "Native '$State' image missing visible '$pattern'. OCR: $($text.text)" }
    }
    $capture.contentVerified = $true
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
    [void][NativeDesktop]::SetForegroundWindow($app.MainWindowHandle)
    Wait-Requests 'normal'
    $probe = Join-Path $root 'probe.png'
    $null = Save-Window $probe
    $text = Read-Window $probe
    $line = @($text.lines | Where-Object { $_.text -match 'fixture.?[o0]rg' } | Select-Object -First 1)
    if ($line.Count -ne 1) { throw "Cannot locate repository for real mouse input. OCR: $($text.text)" }
    $bounds = Window-Rect
    # The beginning of the OCR repository line is in its label, away from linked activity cells.
    $x=$bounds.X+[int]$line[0].x+8; $y=$bounds.Y+[int]($line[0].y+$line[0].height/2)
    [NativeDesktop]::Click($x,$y)
    $manifest.inputs += @{kind='SendInput repository click';x=$x;y=$y;at=[DateTimeOffset]::UtcNow.ToString('o')}
    Start-Sleep -Seconds 2
    Capture-State 'normal' @('SYNTHETIC FIXTURE NORMAL')
    Refresh-State 'stale'
    Capture-State 'stale' @('upstream refresh failed','SYNTHETIC FIXTURE NORMAL')
    Refresh-State 'unavailable'
    Capture-State 'unavailable' @('no.status')
    Refresh-State 'refreshed'
    Capture-State 'refresh' @('SYNTHETIC FIXTURE REFRESHED')
    if ($manifest.captures[-1].sha256 -eq $manifest.captures[-2].sha256) { throw 'OS refresh did not visibly change the image.' }
    Refresh-State 'board'
    Capture-State 'board' @('SYNTHETIC FIXTURE BOARD','Ready','In progress','Waiting on Franz','Blocked','all API pages')
    $manifest.requests = @(Get-Content (Join-Path $root 'requests.jsonl') | ForEach-Object { $_ | ConvertFrom-Json })
    $manifest.success = $true
} catch {
    $manifest.failure = $_.Exception.Message
    throw
} finally {
    # Each owned child is stopped even if another cleanup operation fails.
    $cleanupErrors = @()
    foreach ($owned in @($app,$fixture)) {
        try { Stop-Owned $owned } catch { $cleanupErrors += $_.Exception.Message }
    }
    try { if (Test-Path $root) { Remove-Item $root -Recurse -Force } } catch { $cleanupErrors += $_.Exception.Message }
    if ($cleanupErrors.Count) { $manifest.success=$false; $manifest.cleanupErrors=$cleanupErrors }
    $manifest.finishedAt = [DateTimeOffset]::UtcNow.ToString('o')
    $manifest | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $OutDir 'manifest.json')
    if ($cleanupErrors.Count) { throw "Native attempt cleanup failed: $($cleanupErrors -join '; ')" }
}
