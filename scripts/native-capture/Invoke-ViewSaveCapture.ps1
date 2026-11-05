# Issue 83 on the hosted Windows desktop: the release exe, a real Administrators-owned config.toml and a token with
# Administrators deny-only, which is what Franz's non-elevated app has. Each real click must save its view with no banner.
# Every view is photographed even when an earlier one fails, so a failed run shows the whole problem.
param(
    [string]$Exe = 'target/release/project-dashboard.exe',
    [string]$OutDir = 'view-save-captures',
    [Parameter(Mandatory)][string]$ExpectedHead
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$root = Join-Path ([IO.Path]::GetTempPath()) "dashboard-view-save-$([Guid]::NewGuid().ToString('N'))"
$app = $null; $fixture = $null; $ocr = $null
$failures = [Collections.Generic.List[string]]::new()
$readId = 0
. (Join-Path $PSScriptRoot 'OcrPipe.ps1')
$manifest = [ordered]@{
    head = (git rev-parse HEAD); expectedHead = $ExpectedHead
    runner = 'GitHub-hosted windows-latest'; run = "https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
    syntheticLoopbackFixture = $true; success = $false; views = @(); failures = @()
    startedAt = [DateTimeOffset]::UtcNow.ToString('o')
}

$essentials = @('PATH','SystemRoot','WINDIR','USERPROFILE','APPDATA','LOCALAPPDATA','TEMP','TMP','COMSPEC','PATHEXT','ProgramFiles','ProgramFiles(x86)','ProgramData','SystemDrive')
function Get-Essentials {
    $values = @{}
    foreach ($name in $essentials) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value) { $values[$name] = $value }
    }
    $values
}
function New-HelperStart([string]$File, [string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new($File)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.Environment.Clear()
    $values = Get-Essentials
    foreach ($name in $values.Keys) { $start.Environment[$name] = $values[$name] }
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $start
}
function Stop-Helper($Process) {
    if ($null -ne $Process) {
        if (-not $Process.HasExited) { $Process.Kill($true); [void]$Process.WaitForExit(5000) }
        if (-not $Process.HasExited) { throw "Owned process $($Process.Id) did not stop." }
        $Process.Dispose()
    }
}
function Get-WindowBounds {
    $app.Refresh()
    if ($app.HasExited) { throw 'Dashboard exited during the attempt.' }
    if ($app.MainWindowTitle -cne 'Project Dashboard') { throw 'Owned window title changed.' }
    if ([NativeDesktop]::InputDesktop() -ne 'Default') { throw 'Locked/noninteractive input desktop.' }
    [void][NativeDesktop]::SetForegroundWindow($app.MainWindowHandle)
    $deadline = [DateTime]::UtcNow.AddSeconds(2)
    while ([NativeDesktop]::GetForegroundWindow() -ne $app.MainWindowHandle -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 50
    }
    if ([NativeDesktop]::GetForegroundWindow() -ne $app.MainWindowHandle) { throw 'Dashboard is not foreground.' }
    $rect = [NativeDesktop+RECT]::new()
    if (-not [NativeDesktop]::GetWindowRect($app.MainWindowHandle,[ref]$rect)) { throw 'GetWindowRect failed.' }
    [Drawing.Rectangle]::FromLTRB($rect.Left,$rect.Top,$rect.Right,$rect.Bottom)
}
function Read-Image([string]$Path) {
    $script:readId++
    # The first read also starts the OCR engine, which takes longer.
    (Invoke-OcrRead $ocr $Path $readId $(if ($readId -eq 1) { 120000 } else { 60000 })).result
}
function Save-Window([string]$Name) {
    $bounds = Get-WindowBounds
    $path = Join-Path $OutDir "$Name.png"
    $bitmap = [Drawing.Bitmap]::new($bounds.Width,$bounds.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        # Only GetWindowRect of this owned foreground process, never the desktop.
        $graphics.CopyFromScreen($bounds.Location,[Drawing.Point]::Empty,$bounds.Size)
        $bitmap.Save($path,[Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
    @{bounds=$bounds; path=$path; file="$Name.png"; text=(Read-Image $path)}
}
function Find-Tab($Shot, [string]$Name) {
    # The title bar also says "Project", so anchor on Backlog, the one tab whose word is unique near the top.
    $anchors = @($Shot.text.words | Where-Object { $_.text -ceq 'Backlog' } | Sort-Object y)
    if ($anchors.Count -lt 1) { throw "Cannot find the Backlog tab. OCR: $($Shot.text.text)" }
    $anchor = $anchors[0]
    if ($Name -ceq 'Backlog') { return $anchor }
    $row = @($Shot.text.words | Where-Object { $_.text -ceq $Name -and [Math]::Abs($_.y-$anchor.y) -lt $anchor.height })
    if ($row.Count -ne 1) { throw "Cannot find one $Name tab beside Backlog. OCR: $($Shot.text.text)" }
    $row[0]
}
function Read-SavedView {
    $text = [IO.File]::ReadAllText($configPath)
    if ($text -notmatch "(?m)^view\s*=\s*['""](\w+)['""]") { throw "config.toml has no view line: $text" }
    $Matches[1]
}
function Test-Saved([string]$View) {
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ((Read-SavedView) -cne $View -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    (Read-SavedView) -ceq $View
}
function Assert-View([string]$Step, [string]$View, [string]$Marker, $Shot) {
    $visible = [regex]::Replace($Shot.text.text,'\s+',' ')
    $problem = @()
    if ($visible -match "can.?t write|security ID|os error|1307") { $problem += 'the save-error banner is showing' }
    if (-not (Test-Saved $View)) { $problem += "config.toml still says view = '$(Read-SavedView)'" }
    if ($visible -notmatch $Marker) { $problem += "the $View view's content ($Marker) isn't visible" }
    $security = Get-Acl -LiteralPath $configPath
    $manifest.views += @{
        step=$Step; view=$View; file=$Shot.file; sha256=(Get-FileHash $Shot.path -Algorithm SHA256).Hash.ToLowerInvariant()
        savedView=(Read-SavedView); owner=$security.Owner; visibleText=$Shot.text.text; problems=$problem
    }
    foreach ($item in $problem) { $failures.Add("${Step}: $item") }
    Write-Host "$Step ($View): owner $($security.Owner), $(if ($problem) { $problem -join '; ' } else { 'saved, no banner' })"
}
function Switch-View($From, [string]$Tab, [string]$Step, [string]$View, [string]$Marker, [int]$SettleSeconds) {
    $word = Find-Tab $From $Tab
    $bounds = Get-WindowBounds
    $x = $bounds.X+[int]($word.x+$word.width/2); $y = $bounds.Y+[int]($word.y+$word.height/2)
    if (-not [NativeDesktop]::OwnsPoint($app.MainWindowHandle,$x,$y)) { throw "The $Tab tab isn't inside the Dashboard window." }
    [NativeDesktop]::Click($x,$y)
    Start-Sleep -Seconds $SettleSeconds
    $shot = Save-Window $Step
    Assert-View $Step $View $Marker $shot
    $shot
}

try {
    if (-not $IsWindows) { throw 'This capture requires Windows.' }
    if ($manifest.head -ne $ExpectedHead) { throw 'Checkout is not the PR head.' }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not [Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Creating an Administrators-owned config needs an elevated runner.'
    }
    $Exe = (Resolve-Path $Exe).Path
    $manifest.exeSha256 = (Get-FileHash $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
    Add-Type -Path (Join-Path $PSScriptRoot 'NativeDesktop.cs')
    Add-Type -Path (Join-Path $PSScriptRoot 'FilteredToken.cs')
    Add-Type -AssemblyName System.Windows.Forms,System.Drawing
    [void][NativeDesktop]::SetThreadDpiAwarenessContext([IntPtr](-4))
    $manifest.inputDesktop = [NativeDesktop]::InputDesktop()
    if (-not [Environment]::UserInteractive -or $manifest.inputDesktop -ne 'Default') { throw 'Locked/noninteractive runner desktop.' }
    $area = [Windows.Forms.Screen]::PrimaryScreen.WorkingArea

    New-Item -ItemType Directory -Path (Join-Path $root 'checkout/.git') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $root 'checkout/.git/config'),"[remote `"origin`"]`nurl = https://github.com/fixture-org/synthetic-dashboard.git`n")
    [IO.File]::WriteAllText((Join-Path $root 'read-token'),'synthetic-issue31-token')
    [IO.File]::WriteAllText((Join-Path $root 'state.txt'),'normal')

    $ocrStart = New-HelperStart (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe') @('-NoLogo','-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'Read-WindowText.ps1'))
    $ocrStart.RedirectStandardInput=$true; $ocrStart.RedirectStandardOutput=$true; $ocrStart.RedirectStandardError=$true
    $ocrStart.StandardInputEncoding=[Text.UTF8Encoding]::new($false)
    $ocrStart.StandardOutputEncoding=[Text.UTF8Encoding]::new($false)
    $ocr = [Diagnostics.Process]::Start($ocrStart)
    $null = $ocr.StandardError.ReadToEndAsync()

    $fixture = [Diagnostics.Process]::Start((New-HelperStart (Join-Path $PSHOME 'pwsh.exe') @('-NoProfile','-File',(Join-Path $PSScriptRoot 'Start-LoopbackPmd.ps1'),'-Root',$root)))
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path (Join-Path $root 'port.txt'))) {
        if ($fixture.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'Synthetic loopback fixture did not start within 15 seconds.' }
        Start-Sleep -Milliseconds 100
    }
    $port = (Get-Content (Join-Path $root 'port.txt') -Raw).Trim()

    # The reported shape: Administrators owns the file, the user has full control through inherited entries.
    $configPath = Join-Path $root 'config.toml'
    $configText = @"
# keep this comment
view = 'project'
repositories = ['$(Join-Path $root 'checkout')']
refresh_minutes = 0
parallelism = 1
pmd_url = 'http://127.0.0.1:$port'
pmd_token_path = 'read-token'
t3_home = '$root'
"@
    [IO.File]::WriteAllText($configPath,$configText)
    $administrators = [Security.Principal.SecurityIdentifier]::new([Security.Principal.WellKnownSidType]::BuiltinAdministratorsSid,$null)
    $security = Get-Acl -LiteralPath $configPath
    $security.SetOwner($administrators)
    Set-Acl -LiteralPath $configPath -AclObject $security
    $before = Get-Acl -LiteralPath $configPath
    if ($before.GetOwner([Security.Principal.SecurityIdentifier]) -ne $administrators) { throw "Fixture owner is $($before.Owner), not Administrators." }
    $manifest.ownerBefore = $before.Owner
    $manifest.accessBefore = $before.AccessToString

    $environment = Get-Essentials
    $environment['PROJECT_DASHBOARD_CONFIG_DIR'] = $root
    $app = [Diagnostics.Process]::GetProcessById([FilteredToken]::Start($Exe,$root,$environment))
    $manifest.dashboardProcessId = $app.Id
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 200; $app.Refresh()
        if ($app.HasExited) { throw 'Dashboard exited before showing a window.' }
    } until ($app.MainWindowTitle -eq 'Project Dashboard' -or [DateTime]::UtcNow -ge $deadline)
    if ($app.MainWindowTitle -ne 'Project Dashboard') { throw 'No Dashboard window within 60 seconds.' }
    [void][NativeDesktop]::ShowWindow($app.MainWindowHandle,9)
    if (-not [NativeDesktop]::MoveWindow($app.MainWindowHandle,$area.X+8,$area.Y+8,[Math]::Min(1400,$area.Width-16),[Math]::Min(1000,$area.Height-16),$true)) { throw 'MoveWindow failed.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path (Join-Path $root 'completed-normal')) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (-not (Test-Path (Join-Path $root 'completed-normal'))) { throw 'No production fetch completed within 30 seconds.' }
    Start-Sleep -Seconds 3

    $projectMarker = 'fixture.?[o0]rg'
    $shot = Save-Window '1-project-at-start'
    Assert-View '1-project-at-start' 'project' $projectMarker $shot
    $shot = Switch-View $shot 'Backlog' '2-backlog' 'backlog' 'SYNTHETIC FIXTURE' 2
    # Fleet fetches on selection, and needs time to show its rows.
    $shot = Switch-View $shot 'Fleet' '3-fleet' 'fleet' 'synthetic-windows' 5
    $shot = Switch-View $shot 'Project' '4-project' 'project' $projectMarker 2

    $after = Get-Acl -LiteralPath $configPath
    $manifest.ownerAfter = $after.Owner
    $manifest.accessAfter = $after.AccessToString
    $manifest.groupUnchanged = $after.Group -eq $before.Group
    if ($after.GetOwner([Security.Principal.SecurityIdentifier]) -eq $administrators) { $failures.Add('config.toml is still owned by Administrators, so no save replaced it.') }
    if ($after.AccessToString -cne $before.AccessToString) { $failures.Add('The save changed who can access config.toml.') }
    if ($after.Group -ne $before.Group) { $failures.Add('The save changed the group on config.toml.') }
    # One round trip back to the starting view must leave every byte as it was.
    $saved = [IO.File]::ReadAllText($configPath)
    if ($saved -cne $configText) { $failures.Add("config.toml changed outside its view value: $saved") }
    if ($failures.Count) { throw "Views failed: $($failures -join ' | ')" }
    $manifest.success = $true
} catch {
    $manifest.failure = $_.Exception.Message
    throw
} finally {
    $manifest.failures = @($failures)
    $cleanupErrors = @()
    foreach ($owned in @($app,$fixture,$ocr)) {
        try { Stop-Helper $owned } catch { $cleanupErrors += $_.Exception.Message }
    }
    try { if (Test-Path $root) { Remove-Item $root -Recurse -Force } } catch { $cleanupErrors += $_.Exception.Message }
    if ($cleanupErrors.Count) { $manifest.success=$false; $manifest.cleanupErrors=$cleanupErrors }
    $manifest.finishedAt = [DateTimeOffset]::UtcNow.ToString('o')
    $manifest | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $OutDir 'manifest.json')
    if ($cleanupErrors.Count) { throw "Cleanup failed: $($cleanupErrors -join '; ')" }
}
